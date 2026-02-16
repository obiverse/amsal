/// Trait for audio output backends.
///
/// The engine uses this to abstract over native (cpal) and headless/WASM backends.
/// All methods take `&self` — backends manage their own concurrency.
pub trait AudioBackend: Send + Sync {
    fn play(&self, file_path: &str);
    fn pause(&self);
    fn resume(&self);
    fn stop(&self);
    fn seek(&self, position_ms: u64);
    fn set_volume(&self, volume: f32);
    fn is_playing(&self) -> bool;
    fn is_paused(&self) -> bool;
    fn is_finished(&self) -> bool;
    fn is_error(&self) -> bool;
    fn prepare_next(&self, file_path: &str);
    fn position_ms(&self) -> u64;
    fn duration_ms(&self) -> u64;
}

/// No-op audio backend for headless/WASM use.
///
/// All operations are silent no-ops. Use when you only need
/// library management, playlists, and data operations without audio output.
pub struct NoopBackend;

impl AudioBackend for NoopBackend {
    fn play(&self, _: &str) {}
    fn pause(&self) {}
    fn resume(&self) {}
    fn stop(&self) {}
    fn seek(&self, _: u64) {}
    fn set_volume(&self, _: f32) {}
    fn is_playing(&self) -> bool { false }
    fn is_paused(&self) -> bool { false }
    fn is_finished(&self) -> bool { false }
    fn is_error(&self) -> bool { false }
    fn prepare_next(&self, _: &str) {}
    fn position_ms(&self) -> u64 { 0 }
    fn duration_ms(&self) -> u64 { 0 }
}

#[cfg(feature = "native")]
pub mod audio;
pub mod import;
