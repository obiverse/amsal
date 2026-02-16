//! Audio effect handler — symphonia decode + cpal output.
//!
//! Rust-native audio pipeline:
//! 1. symphonia decodes (MP3, FLAC, AAC, OGG, WAV, ALAC)
//! 2. samples flow through a ring buffer
//! 3. cpal outputs to hardware (CoreAudio / ALSA / WASAPI)
//!
//! Position is tracked via decoded sample count at the known sample rate.
//! Seek is implemented by resetting the decoder to the requested timestamp.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Thread-safe audio effect handler.
pub struct AudioEffect {
    state: Arc<AudioState>,
}

struct AudioState {
    playing: AtomicBool,
    paused: AtomicBool,
    /// Volume 0-100 mapped to 0.0-1.0.
    volume: AtomicU32,
    /// Current position in milliseconds (updated by decoder).
    position_ms: AtomicU64,
    /// Total duration in milliseconds (set when track is probed).
    duration_ms: AtomicU64,
    /// Sample rate of current track.
    sample_rate: AtomicU32,
    /// Channel count of current track.
    channels: AtomicU32,
    /// Channel count the output device is actually configured for.
    output_channels: AtomicU32,
    /// Shared sample buffer: decoder writes, cpal reads.
    samples: Mutex<SampleRing>,
    /// Signal decoder to stop current track.
    stop_signal: AtomicBool,
    /// Seek target in ms (0 = no seek pending).
    seek_to_ms: AtomicU64,
    /// Track finished naturally (end of stream).
    finished: AtomicBool,
    /// Set when decoder or output thread exits with an error.
    error: AtomicBool,
    /// Pre-probed format for next track: (sample_rate, channels, file_path).
    next_probe: Mutex<Option<(u32, u32, String)>>,
    /// Handles for decoder + output threads (joined on stop).
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
}

/// Simple ring buffer for f32 samples.
struct SampleRing {
    buf: Vec<f32>,
    read_pos: usize,
    write_pos: usize,
    len: usize,
}

impl SampleRing {
    fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity],
            read_pos: 0,
            write_pos: 0,
            len: 0,
        }
    }

    fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            if self.len < self.buf.len() {
                self.buf[self.write_pos] = s;
                self.write_pos = (self.write_pos + 1) % self.buf.len();
                self.len += 1;
            }
        }
    }

    fn pull(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.len);
        for sample in out.iter_mut().take(n) {
            *sample = self.buf[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.buf.len();
            self.len -= 1;
        }
        for sample in out.iter_mut().skip(n) {
            *sample = 0.0;
        }
        n
    }

    fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
        self.len = 0;
    }
}

/// Linear interpolation resampler — zero deps, sufficient for playback.
struct LinearResampler {
    ratio: f64,
    phase: f64,
    channels: usize,
}

impl LinearResampler {
    fn new(src_rate: u32, dst_rate: u32, channels: u16) -> Self {
        Self {
            ratio: dst_rate as f64 / src_rate as f64,
            phase: 0.0,
            channels: channels as usize,
        }
    }

    fn is_needed(&self) -> bool {
        (self.ratio - 1.0).abs() > 0.001
    }

    /// Resample interleaved samples. Returns resampled output.
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if !self.is_needed() {
            return input.to_vec();
        }
        let ch = self.channels;
        let in_frames = input.len() / ch;
        if in_frames == 0 {
            return Vec::new();
        }
        let out_frames = ((in_frames as f64) * self.ratio).ceil() as usize;
        let mut output = Vec::with_capacity(out_frames * ch);

        for _ in 0..out_frames {
            let src_idx = self.phase as usize;
            if src_idx >= in_frames {
                break;
            }
            let frac = (self.phase - src_idx as f64) as f32;

            for c in 0..ch {
                let s0 = input[src_idx * ch + c];
                let s1 = if src_idx + 1 < in_frames {
                    input[(src_idx + 1) * ch + c]
                } else {
                    s0
                };
                output.push(s0 + (s1 - s0) * frac);
            }

            self.phase += 1.0 / self.ratio;
        }

        self.phase -= in_frames as f64;
        if self.phase < 0.0 {
            self.phase = 0.0;
        }

        output
    }
}

impl AudioEffect {
    pub fn new() -> Self {
        Self {
            state: Arc::new(AudioState {
                playing: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                volume: AtomicU32::new(80),
                position_ms: AtomicU64::new(0),
                duration_ms: AtomicU64::new(0),
                sample_rate: AtomicU32::new(44100),
                channels: AtomicU32::new(2),
                output_channels: AtomicU32::new(2),
                samples: Mutex::new(SampleRing::new(48000 * 2 * 4)), // ~4s stereo
                stop_signal: AtomicBool::new(false),
                seek_to_ms: AtomicU64::new(0),
                finished: AtomicBool::new(false),
                error: AtomicBool::new(false),
                next_probe: Mutex::new(None),
                threads: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Start playback of a file. Spawns decoder + output threads.
    ///
    /// stop() is called first and blocks until old threads exit — no ghost
    /// threads. The file is probed synchronously so the output stream can
    /// be configured at the track's sample rate.
    pub fn play(&self, file_path: &str) {
        self.stop(); // Blocks until old threads exit

        self.state.stop_signal.store(false, Ordering::SeqCst);
        self.state.playing.store(true, Ordering::SeqCst);
        self.state.paused.store(false, Ordering::SeqCst);
        self.state.finished.store(false, Ordering::SeqCst);
        self.state.error.store(false, Ordering::SeqCst);
        self.state.position_ms.store(0, Ordering::SeqCst);
        self.state.duration_ms.store(0, Ordering::SeqCst);
        self.state.seek_to_ms.store(0, Ordering::SeqCst);
        self.state.samples.lock().clear();

        // Use pre-probed format if available, otherwise probe synchronously
        let probe_result = {
            let mut cached = self.state.next_probe.lock();
            if let Some((r, c, p)) = cached.take() {
                if p == file_path { Some((r, c)) } else { probe_audio_format(file_path) }
            } else {
                probe_audio_format(file_path)
            }
        };
        if let Some((rate, ch)) = probe_result {
            self.state.sample_rate.store(rate, Ordering::SeqCst);
            self.state.channels.store(ch, Ordering::SeqCst);
        }

        let path = file_path.to_string();
        let mut threads = self.state.threads.lock();

        let decoder_state = Arc::clone(&self.state);
        threads.push(thread::spawn(move || {
            if let Err(e) = decode_to_ring(&path, &decoder_state) {
                log::error!("amsal: decode error: {}", e);
                decoder_state.error.store(true, Ordering::SeqCst);
            }
            decoder_state.finished.store(true, Ordering::SeqCst);
        }));

        let output_state = Arc::clone(&self.state);
        threads.push(thread::spawn(move || {
            let err_state = Arc::clone(&output_state);
            if let Err(e) = output_from_ring(output_state) {
                log::error!("amsal: output error: {}", e);
                err_state.error.store(true, Ordering::SeqCst);
            }
        }));
    }

    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.state.stop_signal.store(true, Ordering::SeqCst);
        self.state.playing.store(false, Ordering::SeqCst);
        self.state.paused.store(false, Ordering::SeqCst);
        self.state.samples.lock().clear(); // Clear first so output thread exits fast

        // Drain handles then join outside the lock
        let handles: Vec<_> = self.state.threads.lock().drain(..).collect();
        for handle in handles {
            let _ = handle.join();
        }
    }

    /// Seek to a position in milliseconds.
    pub fn seek(&self, position_ms: u64) {
        self.state.seek_to_ms.store(position_ms, Ordering::SeqCst);
    }

    pub fn set_volume(&self, volume: f32) {
        let v = (volume.clamp(0.0, 1.0) * 100.0) as u32;
        self.state.volume.store(v, Ordering::SeqCst);
    }

    pub fn is_playing(&self) -> bool {
        self.state.playing.load(Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::SeqCst)
    }

    /// Returns true when the current track finished naturally.
    pub fn is_finished(&self) -> bool {
        self.state.finished.load(Ordering::SeqCst)
    }

    /// Returns true when an audio error occurred (decoder or output thread).
    pub fn is_error(&self) -> bool {
        self.state.error.load(Ordering::SeqCst)
    }

    /// Pre-probe the next track's format for faster gapless transitions.
    pub fn prepare_next(&self, file_path: &str) {
        if let Some((rate, ch)) = probe_audio_format(file_path) {
            *self.state.next_probe.lock() = Some((rate, ch, file_path.to_string()));
        }
    }

    pub fn position_ms(&self) -> u64 {
        self.state.position_ms.load(Ordering::SeqCst)
    }

    pub fn duration_ms(&self) -> u64 {
        self.state.duration_ms.load(Ordering::SeqCst)
    }
}

impl Default for AudioEffect {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode a file using symphonia and push samples to the ring buffer.
fn decode_to_ring(file_path: &str, state: &AudioState) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(file_path);
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;
    let track = format.default_track().ok_or("no default track")?;
    let track_id = track.id;

    // Extract sample rate and duration from codec params
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count() as u32).unwrap_or(2);
    state.sample_rate.store(sample_rate, Ordering::SeqCst);
    state.channels.store(channels, Ordering::SeqCst);

    // Compute duration from n_frames if available
    if let Some(n_frames) = track.codec_params.n_frames {
        let duration_ms = (n_frames as u64 * 1000) / sample_rate as u64;
        state.duration_ms.store(duration_ms, Ordering::SeqCst);
    }

    let mut decoder = symphonia::default::get_codecs().make(
        &track.codec_params,
        &DecoderOptions::default(),
    )?;

    // Determine device rate for potential resampling
    let device_rate = probe_device_rate(sample_rate);
    let mut resampler = if device_rate != sample_rate {
        log::info!("amsal: resampling {}Hz -> {}Hz", sample_rate, device_rate);
        Some(LinearResampler::new(sample_rate, device_rate, channels as u16))
    } else {
        None
    };

    let mut decoded_frames: u64 = 0;

    loop {
        if state.stop_signal.load(Ordering::SeqCst) {
            break;
        }

        // Handle seek requests
        let seek_ms = state.seek_to_ms.swap(0, Ordering::SeqCst);
        if seek_ms > 0 {
            let seek_time = Time::new(seek_ms / 1000, (seek_ms % 1000) as f64 / 1000.0);
            if format
                .seek(SeekMode::Accurate, SeekTo::Time { time: seek_time, track_id: Some(track_id) })
                .is_ok()
            {
                decoder.reset();
                state.samples.lock().clear();
                decoded_frames = (seek_ms * sample_rate as u64) / 1000;
                state.position_ms.store(seek_ms, Ordering::SeqCst);
            }
        }

        // Wait while paused
        while state.paused.load(Ordering::SeqCst) {
            if state.stop_signal.load(Ordering::SeqCst) {
                return Ok(());
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }

        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break; // End of stream
            }
            Err(e) => return Err(e.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let n_frames = decoded.frames();

        let mut sample_buf = SampleBuffer::<f32>::new(n_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        let raw_samples: Vec<f32> = sample_buf.samples().to_vec();
        let samples = match resampler.as_mut() {
            Some(rs) => rs.process(&raw_samples),
            None => raw_samples,
        };

        // Update position
        decoded_frames += n_frames as u64;
        let pos_ms = (decoded_frames * 1000) / sample_rate as u64;
        state.position_ms.store(pos_ms, Ordering::SeqCst);

        // Push to ring, back-pressure if full
        loop {
            let mut ring = state.samples.lock();
            let available = ring.buf.len() - ring.len;
            if available >= samples.len() {
                ring.push(&samples);
                break;
            }
            drop(ring);
            thread::sleep(std::time::Duration::from_millis(5));

            if state.stop_signal.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Pull samples from the ring buffer and send to cpal output.
///
/// Takes Arc so the cpal callback closure can hold a safe reference
/// without raw pointers. Output stream is configured at the track's
/// sample rate (probed in play()) to avoid playback-speed drift.
fn output_from_ring(state: Arc<AudioState>) -> Result<(), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no output device")?;

    let track_rate = state.sample_rate.load(Ordering::SeqCst);
    let track_channels = state.channels.load(Ordering::SeqCst).max(1) as u16;

    // Check if device supports the track's rate + channels + f32 format
    let device_supports_track = device
        .supported_output_configs()
        .map(|configs| {
            configs.into_iter().any(|range| {
                range.sample_format() == cpal::SampleFormat::F32
                    && range.channels() >= track_channels
                    && range.min_sample_rate().0 <= track_rate
                    && range.max_sample_rate().0 >= track_rate
            })
        })
        .unwrap_or(false);

    let config: cpal::StreamConfig = if device_supports_track {
        cpal::StreamConfig {
            channels: track_channels,
            sample_rate: cpal::SampleRate(track_rate),
            buffer_size: cpal::BufferSize::Default,
        }
    } else {
        // Verify default config supports f32
        let default_cfg = device.default_output_config()?;
        if default_cfg.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!(
                "device does not support f32 output (got {:?})",
                default_cfg.sample_format()
            ).into());
        }
        default_cfg.into()
    };

    let out_channels = config.channels;
    state.output_channels.store(out_channels as u32, Ordering::SeqCst);

    let cb_state = Arc::clone(&state);
    let stream = device.build_output_stream(
        &config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            if cb_state.paused.load(Ordering::SeqCst) {
                data.fill(0.0);
                return;
            }
            let ring_ch = cb_state.channels.load(Ordering::SeqCst) as u16;
            if ring_ch == out_channels || out_channels == 0 {
                // Channels match — pull directly
                cb_state.samples.lock().pull(data);
            } else {
                // Channel mismatch — pull at ring's channel count, adapt
                let frames = data.len() / out_channels as usize;
                let ring_samples = frames * ring_ch as usize;
                let mut tmp = vec![0.0f32; ring_samples];
                cb_state.samples.lock().pull(&mut tmp);
                adapt_channels(&tmp, ring_ch, data, out_channels);
            }
            let vol = cb_state.volume.load(Ordering::SeqCst) as f32 / 100.0;
            for s in data.iter_mut() {
                *s *= vol;
            }
        },
        move |err| {
            log::error!("amsal: cpal error: {}", err);
        },
        None,
    )?;

    stream.play()?;

    // Keep stream alive while playing or draining
    loop {
        let finished = state.finished.load(Ordering::SeqCst);
        let buffered = state.samples.lock().len;
        let stopped = state.stop_signal.load(Ordering::SeqCst);

        if stopped && buffered == 0 {
            break;
        }
        if finished && buffered == 0 {
            break;
        }
        if !state.playing.load(Ordering::SeqCst) && !finished && buffered == 0 {
            break;
        }

        thread::sleep(std::time::Duration::from_millis(25));
    }

    state.playing.store(false, Ordering::SeqCst);
    Ok(())
}

/// Adapt interleaved samples between different channel counts.
/// Handles mono→stereo, stereo→mono, and general up/down-mix.
fn adapt_channels(src: &[f32], src_ch: u16, dst: &mut [f32], dst_ch: u16) {
    let src_ch = src_ch as usize;
    let dst_ch = dst_ch as usize;
    let frames = dst.len() / dst_ch;

    for f in 0..frames {
        let src_off = f * src_ch;
        let dst_off = f * dst_ch;

        if src_ch == 1 && dst_ch >= 2 {
            // Mono → stereo+: duplicate to all channels
            let s = if src_off < src.len() { src[src_off] } else { 0.0 };
            for c in 0..dst_ch {
                dst[dst_off + c] = s;
            }
        } else if src_ch >= 2 && dst_ch == 1 {
            // Stereo+ → mono: average all source channels
            let mut sum = 0.0f32;
            let n = src_ch.min(src.len().saturating_sub(src_off));
            for c in 0..n {
                sum += src[src_off + c];
            }
            dst[dst_off] = if n > 0 { sum / n as f32 } else { 0.0 };
        } else {
            // General: copy matching channels, zero-fill extra, drop excess
            let copy_ch = src_ch.min(dst_ch);
            for c in 0..copy_ch {
                dst[dst_off + c] = if src_off + c < src.len() { src[src_off + c] } else { 0.0 };
            }
            for c in copy_ch..dst_ch {
                dst[dst_off + c] = 0.0;
            }
        }
    }
}

/// Determine what sample rate the output device will use.
/// If the device supports the track rate, use that. Otherwise fall back to default.
fn probe_device_rate(track_rate: u32) -> u32 {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return track_rate;
    };

    let supports_track = device
        .supported_output_configs()
        .map(|configs| {
            configs.into_iter().any(|range| {
                range.min_sample_rate().0 <= track_rate
                    && range.max_sample_rate().0 >= track_rate
            })
        })
        .unwrap_or(false);

    if supports_track {
        track_rate
    } else {
        device
            .default_output_config()
            .map(|c| c.sample_rate().0)
            .unwrap_or(track_rate)
    }
}

/// Probe a file's audio format without decoding. Returns (sample_rate, channels).
fn probe_audio_format(file_path: &str) -> Option<(u32, u32)> {
    let path = Path::new(file_path);
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;

    let track = probed.format.default_track()?;
    let rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count() as u32).unwrap_or(2);
    Some((rate, channels))
}

#[cfg(test)]
mod tests {
    use super::{LinearResampler, SampleRing};

    #[test]
    fn push_pull_roundtrip() {
        let mut ring = SampleRing::new(16);
        ring.push(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = [0.0f32; 4];
        let n = ring.pull(&mut out);
        assert_eq!(n, 4);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn overflow_drops_excess() {
        let mut ring = SampleRing::new(4);
        ring.push(&[1.0, 2.0, 3.0, 4.0]);
        ring.push(&[5.0, 6.0]); // Full — silently dropped
        assert_eq!(ring.len, 4);
        let mut out = [0.0f32; 6];
        let n = ring.pull(&mut out);
        assert_eq!(n, 4);
        assert_eq!(&out[..4], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(&out[4..], &[0.0, 0.0]);
    }

    #[test]
    fn underflow_fills_zeros() {
        let mut ring = SampleRing::new(8);
        ring.push(&[1.0, 2.0]);
        let mut out = [0.0f32; 6];
        let n = ring.pull(&mut out);
        assert_eq!(n, 2);
        assert_eq!(&out[..2], &[1.0, 2.0]);
        assert_eq!(&out[2..], &[0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn clear_resets() {
        let mut ring = SampleRing::new(8);
        ring.push(&[1.0, 2.0, 3.0]);
        ring.clear();
        assert_eq!(ring.len, 0);
        let mut out = [0.0f32; 2];
        let n = ring.pull(&mut out);
        assert_eq!(n, 0);
    }

    #[test]
    fn interleaved_push_pull() {
        let mut ring = SampleRing::new(8);
        ring.push(&[1.0, 2.0, 3.0]);
        let mut out = [0.0f32; 2];
        ring.pull(&mut out);
        assert_eq!(out, [1.0, 2.0]);
        ring.push(&[4.0, 5.0]);
        let mut out2 = [0.0f32; 3];
        let n = ring.pull(&mut out2);
        assert_eq!(n, 3);
        assert_eq!(out2, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn wraparound_behavior() {
        let mut ring = SampleRing::new(4);
        ring.push(&[1.0, 2.0, 3.0]);
        let mut out = [0.0f32; 3];
        ring.pull(&mut out);
        // read_pos=3, write_pos=3 — next push wraps around
        ring.push(&[7.0, 8.0, 9.0, 10.0]);
        let mut out2 = [0.0f32; 4];
        ring.pull(&mut out2);
        assert_eq!(out2, [7.0, 8.0, 9.0, 10.0]);
    }

    #[test]
    fn resampler_same_rate_passthrough() {
        let mut rs = LinearResampler::new(44100, 44100, 2);
        assert!(!rs.is_needed());
        let input = vec![1.0, 2.0, 3.0, 4.0];
        let output = rs.process(&input);
        assert_eq!(output, input);
    }

    #[test]
    fn resampler_upsample_produces_more() {
        let mut rs = LinearResampler::new(22050, 44100, 1);
        assert!(rs.is_needed());
        let input = vec![0.0, 1.0, 0.0, -1.0];
        let output = rs.process(&input);
        assert!(output.len() > input.len());
    }

    #[test]
    fn resampler_downsample_produces_fewer() {
        let mut rs = LinearResampler::new(96000, 48000, 1);
        assert!(rs.is_needed());
        let input: Vec<f32> = (0..96).map(|i| i as f32 / 96.0).collect();
        let output = rs.process(&input);
        assert!(output.len() < input.len());
    }

    #[test]
    fn adapt_mono_to_stereo() {
        let src = [1.0, 2.0, 3.0]; // 3 mono frames
        let mut dst = [0.0f32; 6]; // 3 stereo frames
        super::adapt_channels(&src, 1, &mut dst, 2);
        assert_eq!(dst, [1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
    }

    #[test]
    fn adapt_stereo_to_mono() {
        let src = [1.0, 3.0, 2.0, 4.0]; // 2 stereo frames
        let mut dst = [0.0f32; 2]; // 2 mono frames
        super::adapt_channels(&src, 2, &mut dst, 1);
        assert_eq!(dst, [2.0, 3.0]); // averages
    }

    #[test]
    fn adapt_same_channels_passthrough() {
        let src = [1.0, 2.0, 3.0, 4.0]; // 2 stereo frames
        let mut dst = [0.0f32; 4];
        super::adapt_channels(&src, 2, &mut dst, 2);
        assert_eq!(dst, [1.0, 2.0, 3.0, 4.0]);
    }
}
