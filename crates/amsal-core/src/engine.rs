//! Amsal engine — effect kernel over 9S scrolls.
//!
//! Owns a Shell + AudioEffect. Runs effect handlers in background threads
//! that watch scroll paths and dispatch side effects.
//!
//! Fixes over v1:
//! - Mutex-guarded playback state (no more race conditions)
//! - Shutdown lifecycle (no more ghost threads)
//! - Effect registry pattern (composable handlers)

use nine_s_core::errors::NineSResult;
use nine_s_core::scroll::Scroll;
use nine_s_shell::Shell;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beeclock_core::Clock;

#[cfg(feature = "native")]
use crate::effects::audio::AudioEffect;
use crate::effects::AudioBackend;
use crate::effects::import;
use crate::models::playback::PlaybackCommand;
use crate::models::scroll_ext::{
    default_playback_state, default_queue_state, queue_current_id, repeat_mode, ScrollExt,
};
use crate::paths;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// The amsal media engine.
pub struct Engine {
    shell: Arc<Shell>,
    audio: Arc<dyn AudioBackend>,
    /// Authoritative playback state — protected by mutex.
    /// Written to scroll as a side-effect for watchers.
    state: Arc<Mutex<Value>>,
    /// Authoritative queue state — mirrors to scroll as side-effect.
    queue: Arc<Mutex<Value>>,
    /// Shutdown signal for all background threads.
    shutdown: Arc<AtomicBool>,
    /// Handles for joining background threads.
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl Engine {
    /// Boot the engine with a 9S shell and the native (cpal) audio backend.
    #[cfg(feature = "native")]
    pub fn new(shell: Shell) -> Self {
        Self::with_backend(shell, Arc::new(AudioEffect::new()))
    }

    /// Boot the engine with a custom audio backend.
    ///
    /// Use `NoopBackend` for headless/WASM (data-only, no audio output).
    pub fn with_backend(shell: Shell, audio: Arc<dyn AudioBackend>) -> Self {
        let initial_state = default_playback_state();
        let initial_queue = default_queue_state();

        log_err(shell.put(paths::PLAYBACK_STATE, initial_state.clone()), "init playback state");
        log_err(shell.put(paths::QUEUE_CURRENT, initial_queue.clone()), "init queue state");

        Self {
            shell: Arc::new(shell),
            audio,
            state: Arc::new(Mutex::new(initial_state)),
            queue: Arc::new(Mutex::new(initial_queue)),
            shutdown: Arc::new(AtomicBool::new(false)),
            handles: Mutex::new(Vec::new()),
        }
    }

    /// Start all effect loops. Idempotent — calling twice is a no-op.
    pub fn start(&self) {
        let mut handles = self.handles.lock();
        if !handles.is_empty() {
            return;
        }
        handles.push(self.start_playback_loop());
        handles.push(self.start_import_loop());
        handles.push(self.start_heartbeat());
    }

    /// Stop all effect loops and wait for them to finish.
    ///
    /// Sentinel writes unblock rx.iter() watchers so they see the
    /// shutdown flag. Without this, join() deadlocks.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.audio.stop();

        // Wake blocked watchers by writing sentinel scrolls
        log_err(self.shell.put(paths::PLAYBACK_COMMAND, serde_json::json!({"action": "noop"})), "shutdown sentinel playback");
        log_err(self.shell.put(paths::IMPORT_REQUEST, serde_json::json!({"shutdown": true})), "shutdown sentinel import");

        let mut handles = self.handles.lock();
        for handle in handles.drain(..) {
            let _ = handle.join();
        }
    }

    // -----------------------------------------------------------------------
    // Effect loops (return JoinHandles)
    // -----------------------------------------------------------------------

    fn start_playback_loop(&self) -> JoinHandle<()> {
        let shell = Arc::clone(&self.shell);
        let audio = Arc::clone(&self.audio);
        let state = Arc::clone(&self.state);
        let queue = Arc::clone(&self.queue);
        let shutdown = Arc::clone(&self.shutdown);

        thread::spawn(move || {
            let rx = match shell.on(paths::PLAYBACK_COMMAND) {
                Ok(rx) => rx,
                Err(e) => {
                    log::error!("amsal: failed to watch playback commands: {}", e);
                    return;
                }
            };

            for scroll in rx.iter() {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(cmd) = PlaybackCommand::from_value(&scroll.data) {
                    handle_playback(&shell, &*audio, &state, &queue, cmd);
                }
            }
        })
    }

    fn start_import_loop(&self) -> JoinHandle<()> {
        let shell = Arc::clone(&self.shell);
        let shutdown = Arc::clone(&self.shutdown);

        thread::spawn(move || {
            let rx = match shell.on(paths::IMPORT_REQUEST) {
                Ok(rx) => rx,
                Err(e) => {
                    log::error!("amsal: failed to watch import requests: {}", e);
                    return;
                }
            };

            for scroll in rx.iter() {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(dir) = scroll.data["dir"].as_str() {
                    log_err(shell.put(
                        paths::IMPORT_STATUS,
                        serde_json::json!({"scanning": true, "dir": dir}),
                    ), "import status scanning");

                    let imported = import::scan_directory(&shell, dir);

                    log_err(shell.put(
                        paths::IMPORT_STATUS,
                        serde_json::json!({
                            "scanning": false,
                            "imported": imported,
                            "dir": dir,
                        }),
                    ), "import status complete");
                } else if let Some(file) = scroll.data["file"].as_str() {
                    import::import_file(&shell, file);
                }
            }
        })
    }

    /// Clock-driven heartbeat — replaces ad-hoc position polling.
    ///
    /// A BeeClock with musical partitions (sub/beat/bar) ticks at 4 Hz.
    /// Each tick: sync audio position, check track end, write clock state
    /// to scrolls. Pulses fire at structural intervals — Flutter/web
    /// watches `/amsal/clock/**` to drive animations and game-like flows.
    fn start_heartbeat(&self) -> JoinHandle<()> {
        let shell = Arc::clone(&self.shell);
        let audio = Arc::clone(&self.audio);
        let state = Arc::clone(&self.state);
        let queue = Arc::clone(&self.queue);
        let shutdown = Arc::clone(&self.shutdown);

        thread::spawn(move || {
            let mut clock = build_clock(&shell);

            while !shutdown.load(Ordering::SeqCst) {
                thread::sleep(std::time::Duration::from_millis(250));

                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                let outcome = clock.tick();

                // --- Audio error recovery ---
                if audio.is_error() {
                    audio.stop();
                    update_state(&shell, &state, |s| {
                        s["playing"] = false.into();
                        s["error"] = "audio_device_or_decode_error".into();
                    });
                    continue;
                }

                // --- Audio state sync (same semantics as old position tracker) ---
                if !audio.is_playing() && !audio.is_paused() {
                    if audio.is_finished() {
                        // Record play event before advancing to next track
                        let current_id = {
                            let s = state.lock();
                            s["current_id"].as_str().map(String::from)
                        };
                        if let Some(id) = current_id {
                            record_play_event(&shell, &id, audio.position_ms());
                        }
                        advance_queue(&shell, &*audio, &state, &queue);
                    }
                } else {
                    let pos = audio.position_ms();
                    let dur = audio.duration_ms();

                    update_state(&shell, &state, |s| {
                        s["position_ms"] = pos.into();
                        if dur > 0 {
                            s["duration_ms"] = dur.into();
                        }
                        s["playing"] = (audio.is_playing() && !audio.is_paused()).into();
                    });

                    // --- Pre-probe next track 3s before end for gapless ---
                    if audio.is_playing() && !audio.is_paused() && dur > 3000 && pos > dur - 3000 {
                        let next_path = {
                            // Lock ordering: state before queue (matches advance_queue)
                            let repeat = {
                                let s = state.lock();
                                repeat_mode(&s).to_string()
                            };
                            let q = queue.lock();
                            if repeat == "one" {
                                None // Will replay same track
                            } else {
                                let items = q["items"].as_array();
                                let len = items.map(|a| a.len()).unwrap_or(0);
                                let idx = q["index"].as_u64().unwrap_or(0) as usize;
                                let next_idx = if idx + 1 < len {
                                    Some(idx + 1)
                                } else if repeat == "all" {
                                    Some(0)
                                } else {
                                    None
                                };
                                next_idx.and_then(|i| {
                                    if q["shuffle"].as_bool().unwrap_or(false) {
                                        q["shuffle_order"]
                                            .as_array()
                                            .and_then(|order| order.get(i))
                                            .and_then(|v| v.as_u64())
                                            .and_then(|actual| items?.get(actual as usize))
                                            .and_then(|v| v.as_str())
                                            .map(String::from)
                                    } else {
                                        items?.get(i)?.as_str().map(String::from)
                                    }
                                })
                            }
                        };
                        if let Some(next_id) = next_path {
                            if let Ok(Some(scroll)) = shell.get(&paths::library_path(&next_id)) {
                                if let Some(fp) = scroll.data["path"].as_str() {
                                    audio.prepare_next(fp);
                                }
                            }
                        }
                    }
                }

                // --- Clock tick → scroll (watchers drive UI/animations) ---
                log_err(shell.put(paths::CLOCK_TICK, tick_to_json(&outcome)), "clock tick");

                // --- Fired pulses → individual scroll paths ---
                for pulse in &outcome.pulses {
                    log_err(shell.put(
                        &paths::clock_pulse_path(&pulse.name),
                        serde_json::json!({
                            "name": &pulse.name,
                            "tick": pulse.tick,
                            "epoch": pulse.epoch,
                        }),
                    ), "clock pulse");
                }
            }
        })
    }

    // -----------------------------------------------------------------------
    // Public API — all scroll operations
    // -----------------------------------------------------------------------

    /// Get reference to the 9S shell.
    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    /// Get reference to the audio backend.
    pub fn audio(&self) -> &dyn AudioBackend {
        &*self.audio
    }

    /// Add a media item to the library. `data` is the item JSON.
    pub fn add_to_library(&self, id: &str, data: Value) -> NineSResult<Scroll> {
        self.shell.put(&paths::library_path(id), data)
    }

    /// List all non-deleted media item paths in the library.
    pub fn list_library(&self) -> NineSResult<Vec<String>> {
        let all_paths = self.shell.all(paths::LIBRARY_PREFIX)?;
        let mut live = Vec::with_capacity(all_paths.len());
        for path in all_paths {
            if let Ok(Some(scroll)) = self.shell.get(&path) {
                if scroll.metadata.deleted != Some(true) {
                    live.push(path);
                }
            }
        }
        Ok(live)
    }

    /// Soft-delete a library item (marks metadata.deleted=true).
    pub fn delete_from_library(&self, id: &str) -> NineSResult<Scroll> {
        let path = paths::library_path(id);
        match self.shell.get(&path)? {
            Some(mut scroll) => {
                scroll.metadata.deleted = Some(true);
                self.shell.put_scroll(scroll)
            }
            None => Err(nine_s_core::errors::NineSError::Other(
                format!("library item not found: {}", id),
            )),
        }
    }

    /// Search library by case-insensitive substring across title, artist, album, genre.
    pub fn search_library(&self, query: &str) -> Vec<Value> {
        let q = query.to_lowercase();
        self.shell
            .all(paths::LIBRARY_PREFIX)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| {
                let scroll = self.shell.get(&path).ok()??;
                if scroll.metadata.deleted == Some(true) {
                    return None;
                }
                let d = &scroll.data;
                let matches = ["title", "artist", "album", "genre"]
                    .iter()
                    .any(|field| {
                        d[*field]
                            .as_str()
                            .map(|v| v.to_lowercase().contains(&q))
                            .unwrap_or(false)
                    });
                if matches { Some(scroll.data) } else { None }
            })
            .collect()
    }

    /// Filter library by case-insensitive exact match on a specific field.
    pub fn filter_library(&self, field: &str, value: &str) -> Vec<Value> {
        let v = value.to_lowercase();
        self.shell
            .all(paths::LIBRARY_PREFIX)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| {
                let scroll = self.shell.get(&path).ok()??;
                if scroll.metadata.deleted == Some(true) {
                    return None;
                }
                let field_val = scroll.data[field].as_str()?;
                if field_val.to_lowercase() == v {
                    Some(scroll.data)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Send a playback command.
    pub fn command(&self, cmd: PlaybackCommand) -> NineSResult<Scroll> {
        self.shell.put(paths::PLAYBACK_COMMAND, cmd.to_value())
    }

    /// Read current playback state (from authoritative mutex, not scroll).
    pub fn playback_state(&self) -> Value {
        self.state.lock().clone()
    }

    /// Read current queue state (from authoritative mutex).
    pub fn queue_state(&self) -> Option<Value> {
        Some(self.queue.lock().clone())
    }

    /// Set the queue.
    pub fn set_queue(&self, items: Vec<String>, start_index: usize) -> NineSResult<Scroll> {
        let new_queue = serde_json::json!({
            "items": items,
            "index": start_index,
            "shuffle": false
        });
        *self.queue.lock() = new_queue.clone();
        self.shell.put(paths::QUEUE_CURRENT, new_queue)
    }

    /// Set favorites (list of media IDs).
    pub fn set_favorites(&self, ids: &[String]) -> NineSResult<Scroll> {
        self.shell
            .put(paths::FAVORITES, serde_json::json!({ "ids": ids }))
    }

    /// Get favorites.
    pub fn favorites(&self) -> Vec<String> {
        self.shell
            .get(paths::FAVORITES)
            .ok()
            .flatten()
            .map(|s| {
                s.data
                    .str_array("ids")
                    .into_iter()
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Request a directory scan for media files.
    pub fn import_dir(&self, dir: &str) -> NineSResult<Scroll> {
        self.shell
            .put(paths::IMPORT_REQUEST, serde_json::json!({"dir": dir}))
    }

    /// Request a single file import.
    pub fn import_file(&self, file: &str) -> NineSResult<Scroll> {
        self.shell
            .put(paths::IMPORT_REQUEST, serde_json::json!({"file": file}))
    }

    /// Read the latest clock tick state from scroll.
    pub fn clock_state(&self) -> Option<Value> {
        self.shell
            .get(paths::CLOCK_TICK)
            .ok()
            .flatten()
            .map(|s| s.data)
    }

    // -------------------------------------------------------------------
    // Album Art
    // -------------------------------------------------------------------

    /// Read album art for a library item. Returns art data JSON or None.
    pub fn album_art(&self, id: &str) -> Option<Value> {
        self.shell
            .get(&paths::art_path(id))
            .ok()
            .flatten()
            .map(|s| s.data)
    }

    // -------------------------------------------------------------------
    // Playlists
    // -------------------------------------------------------------------

    /// Create a new playlist with the given ID and name.
    pub fn create_playlist(&self, id: &str, name: &str) -> NineSResult<Scroll> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.shell.put(
            &paths::playlist_path(id),
            serde_json::json!({
                "id": id,
                "name": name,
                "items": [],
                "created_ms": now,
            }),
        )
    }

    /// Read a playlist by ID. Returns None if not found or deleted.
    pub fn playlist(&self, id: &str) -> Option<Value> {
        self.shell
            .get(&paths::playlist_path(id))
            .ok()
            .flatten()
            .filter(|s| s.metadata.deleted != Some(true))
            .map(|s| s.data)
    }

    /// List all non-deleted playlist paths.
    pub fn list_playlists(&self) -> Vec<String> {
        self.shell
            .all(paths::PLAYLISTS_PREFIX)
            .unwrap_or_default()
            .into_iter()
            .filter(|path| {
                self.shell
                    .get(path)
                    .ok()
                    .flatten()
                    .map(|s| s.metadata.deleted != Some(true))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Add a media item to a playlist.
    pub fn add_to_playlist(&self, playlist_id: &str, media_id: &str) -> NineSResult<Scroll> {
        let path = paths::playlist_path(playlist_id);
        match self.shell.get(&path)? {
            Some(mut scroll) => {
                if let Some(arr) = scroll.data["items"].as_array_mut() {
                    arr.push(serde_json::Value::String(media_id.to_string()));
                }
                self.shell.put(&path, scroll.data)
            }
            None => Err(nine_s_core::errors::NineSError::Other(
                format!("playlist not found: {}", playlist_id),
            )),
        }
    }

    /// Remove a media item from a playlist.
    pub fn remove_from_playlist(&self, playlist_id: &str, media_id: &str) -> NineSResult<Scroll> {
        let path = paths::playlist_path(playlist_id);
        match self.shell.get(&path)? {
            Some(mut scroll) => {
                if let Some(arr) = scroll.data["items"].as_array_mut() {
                    arr.retain(|v| v.as_str() != Some(media_id));
                }
                self.shell.put(&path, scroll.data)
            }
            None => Err(nine_s_core::errors::NineSError::Other(
                format!("playlist not found: {}", playlist_id),
            )),
        }
    }

    /// Soft-delete a playlist.
    pub fn delete_playlist(&self, id: &str) -> NineSResult<Scroll> {
        let path = paths::playlist_path(id);
        match self.shell.get(&path)? {
            Some(mut scroll) => {
                scroll.metadata.deleted = Some(true);
                self.shell.put_scroll(scroll)
            }
            None => Err(nine_s_core::errors::NineSError::Other(
                format!("playlist not found: {}", id),
            )),
        }
    }

    /// Rename a playlist.
    pub fn rename_playlist(&self, id: &str, new_name: &str) -> NineSResult<Scroll> {
        let path = paths::playlist_path(id);
        match self.shell.get(&path)? {
            Some(mut scroll) => {
                scroll.data["name"] = new_name.into();
                self.shell.put(&path, scroll.data)
            }
            None => Err(nine_s_core::errors::NineSError::Other(
                format!("playlist not found: {}", id),
            )),
        }
    }

    // -------------------------------------------------------------------
    // History & Stats
    // -------------------------------------------------------------------

    /// Record a play event (writes history scroll + updates stats).
    pub fn record_play(&self, media_id: &str, duration_played_ms: u64) {
        record_play_event(&self.shell, media_id, duration_played_ms);
    }

    /// Get recent play history, most recent first.
    pub fn play_history(&self, limit: usize) -> Vec<Value> {
        let mut entries: Vec<(String, Value)> = self
            .shell
            .all(paths::HISTORY_PREFIX)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| {
                let scroll = self.shell.get(&path).ok()??;
                Some((path, scroll.data))
            })
            .collect();
        entries.sort_by(|a, b| b.0.cmp(&a.0));
        entries.into_iter().take(limit).map(|(_, v)| v).collect()
    }

    /// Get stats for a single media item.
    pub fn media_stats(&self, id: &str) -> Option<Value> {
        self.shell
            .get(&paths::stats_path(id))
            .ok()
            .flatten()
            .map(|s| s.data)
    }

    /// Get top played items sorted by play_count descending.
    pub fn top_played(&self, limit: usize) -> Vec<Value> {
        let mut entries: Vec<Value> = self
            .shell
            .all(paths::STATS_PREFIX)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| {
                let scroll = self.shell.get(&path).ok()??;
                Some(scroll.data)
            })
            .collect();
        entries.sort_by(|a, b| {
            let ca = b["play_count"].as_u64().unwrap_or(0);
            let cb = a["play_count"].as_u64().unwrap_or(0);
            ca.cmp(&cb)
        });
        entries.into_iter().take(limit).collect()
    }

    // -------------------------------------------------------------------
    // Clock Config
    // -------------------------------------------------------------------

    /// Write clock configuration. Takes effect on next engine start().
    pub fn configure_clock(&self, config: Value) -> NineSResult<Scroll> {
        self.shell.put(paths::CLOCK_CONFIG, config)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.audio.stop();
        // Don't join here — threads will exit when channels close
    }
}

// ---------------------------------------------------------------------------
// Clock configuration
// ---------------------------------------------------------------------------

/// Build a beeclock from scroll config, falling back to defaults.
fn build_clock(shell: &Shell) -> Clock {
    if let Ok(Some(scroll)) = shell.get(paths::CLOCK_CONFIG) {
        if let Some(clock) = try_build_clock_from_config(&scroll.data) {
            return clock;
        }
        log::warn!("amsal: invalid clock config, using defaults");
    }
    default_clock()
}

fn default_clock() -> Clock {
    Clock::builder()
        .least_significant_first()
        .partition("sub", 4)
        .partition("beat", 4)
        .partition("bar", 4)
        .pulse_every("beat", 4)
        .pulse_every("bar", 16)
        .pulse_every("phrase", 64)
        .build()
        .expect("amsal: default clock build failed")
}

fn try_build_clock_from_config(config: &Value) -> Option<Clock> {
    let partitions = config["partitions"].as_array()?;
    let pulses = config["pulses"].as_array()?;

    let mut builder = Clock::builder().least_significant_first();

    for p in partitions {
        let name = p["name"].as_str()?;
        let modulus = p["modulus"].as_u64()?;
        if modulus == 0 {
            return None;
        }
        builder = builder.partition(name, modulus);
    }

    for p in pulses {
        let name = p["name"].as_str()?;
        let every = p["every"].as_u64()?;
        if every == 0 {
            return None;
        }
        builder = builder.pulse_every(name, every);
    }

    builder.build().ok()
}

// ---------------------------------------------------------------------------
// Atomic state mutation — THE fix for the race condition
// ---------------------------------------------------------------------------

/// Log errors from scroll operations without panicking.
fn log_err<T, E: std::fmt::Display>(result: Result<T, E>, context: &str) -> bool {
    match result {
        Ok(_) => true,
        Err(e) => {
            log::warn!("amsal: {} failed: {}", context, e);
            false
        }
    }
}

/// Mutate the authoritative state under lock, then sync to scroll.
fn update_state(shell: &Shell, state: &Mutex<Value>, f: impl FnOnce(&mut Value)) {
    let mut guard = state.lock();
    f(&mut guard);
    log_err(shell.put(paths::PLAYBACK_STATE, guard.clone()), "sync playback state");
}

/// Replace the entire state, then sync to scroll.
fn replace_state(shell: &Shell, state: &Mutex<Value>, new: Value) {
    let mut guard = state.lock();
    *guard = new.clone();
    log_err(shell.put(paths::PLAYBACK_STATE, new), "replace playback state");
}

/// Mutate the authoritative queue under lock, then sync to scroll.
fn update_queue(shell: &Shell, queue: &Mutex<Value>, f: impl FnOnce(&mut Value)) {
    let mut guard = queue.lock();
    f(&mut guard);
    log_err(shell.put(paths::QUEUE_CURRENT, guard.clone()), "sync queue");
}

/// Record a play event: write history scroll + update stats.
fn record_play_event(shell: &Shell, media_id: &str, duration_played_ms: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    log_err(
        shell.put(
            &paths::history_path(now),
            serde_json::json!({
                "media_id": media_id,
                "played_at_ms": now,
                "duration_played_ms": duration_played_ms,
            }),
        ),
        "record history",
    );

    let stats_path = paths::stats_path(media_id);
    let (play_count, total_ms) = shell
        .get(&stats_path)
        .ok()
        .flatten()
        .map(|s| (
            s.data["play_count"].as_u64().unwrap_or(0),
            s.data["total_played_ms"].as_u64().unwrap_or(0),
        ))
        .unwrap_or((0, 0));

    log_err(
        shell.put(
            &stats_path,
            serde_json::json!({
                "media_id": media_id,
                "play_count": play_count + 1,
                "total_played_ms": total_ms + duration_played_ms,
                "last_played_ms": now,
            }),
        ),
        "update stats",
    );
}

// ---------------------------------------------------------------------------
// Effect handlers (pure functions)
// ---------------------------------------------------------------------------

fn handle_playback(shell: &Shell, audio: &dyn AudioBackend, state: &Mutex<Value>, queue: &Mutex<Value>, cmd: PlaybackCommand) {
    match cmd {
        PlaybackCommand::Play { ref id } => {
            let path = paths::library_path(id);
            if let Ok(Some(scroll)) = shell.get(&path) {
                if let Some(file_path) = scroll.data["path"].as_str() {
                    audio.play(file_path);
                    let d = &scroll.data;
                    let duration = d["duration_ms"].as_u64().unwrap_or(0);
                    let title = d["title"].as_str().unwrap_or("Unknown");
                    let artist = d["artist"].as_str().unwrap_or("Unknown");
                    let album = d["album"].as_str().unwrap_or("");
                    // Single snapshot — no interleaved mutations
                    let guard = state.lock();
                    let volume = guard["volume"].as_f64().unwrap_or(0.8);
                    let shuffle = guard["shuffle"].as_bool().unwrap_or(false);
                    let repeat = guard["repeat"].as_str().unwrap_or("off").to_string();
                    drop(guard);
                    replace_state(
                        shell,
                        state,
                        serde_json::json!({
                            "current_id": id,
                            "title": title,
                            "artist": artist,
                            "album": album,
                            "playing": true,
                            "position_ms": 0,
                            "duration_ms": duration,
                            "volume": volume,
                            "shuffle": shuffle,
                            "repeat": repeat,
                        }),
                    );
                }
            }
        }
        PlaybackCommand::Pause => {
            audio.pause();
            update_state(shell, state, |s| s["playing"] = false.into());
        }
        PlaybackCommand::Resume => {
            audio.resume();
            update_state(shell, state, |s| s["playing"] = true.into());
        }
        PlaybackCommand::Stop => {
            audio.stop();
            replace_state(shell, state, default_playback_state());
        }
        PlaybackCommand::Seek { position_ms } => {
            audio.seek(position_ms);
            update_state(shell, state, |s| s["position_ms"] = position_ms.into());
        }
        PlaybackCommand::SetVolume { volume } => {
            audio.set_volume(volume);
            update_state(shell, state, |s| s["volume"] = volume.into());
        }
        PlaybackCommand::Next => {
            advance_queue(shell, audio, state, queue);
        }
        PlaybackCommand::Previous => {
            let pos = audio.position_ms();
            if pos > 3000 {
                audio.seek(0);
                update_state(shell, state, |s| s["position_ms"] = 0.into());
            } else {
                retreat_queue(shell, audio, state, queue);
            }
        }
        PlaybackCommand::SetShuffle { enabled } => {
            update_state(shell, state, |s| s["shuffle"] = enabled.into());
            update_queue(shell, queue, |data| {
                data["shuffle"] = enabled.into();
                if enabled {
                    let len = data["items"]
                        .as_array()
                        .map(|a| a.len())
                        .unwrap_or(0);
                    let idx = data["index"].as_u64().unwrap_or(0) as usize;
                    data["shuffle_order"] = serde_json::to_value(
                        generate_shuffle_order(len, idx),
                    )
                    .unwrap_or_default();
                    data["index"] = 0.into();
                } else {
                    // Resolve actual item index before removing shuffle order
                    if let Some(order) = data["shuffle_order"].as_array() {
                        let shuffle_idx = data["index"].as_u64().unwrap_or(0) as usize;
                        if let Some(actual) = order.get(shuffle_idx).and_then(|v| v.as_u64()) {
                            data["index"] = actual.into();
                        }
                    }
                    data.as_object_mut()
                        .map(|o| o.remove("shuffle_order"));
                }
            });
        }
        PlaybackCommand::SetRepeat { mode } => {
            let mode_str = serde_json::to_value(mode).unwrap_or("off".into());
            update_state(shell, state, |s| s["repeat"] = mode_str);
        }
    }
}

fn advance_queue(shell: &Shell, audio: &dyn AudioBackend, state: &Mutex<Value>, queue: &Mutex<Value>) {
    // Read repeat mode from state (lock ordering: state before queue)
    let repeat = {
        let guard = state.lock();
        repeat_mode(&guard).to_string()
    };

    // Lock queue, compute next index, sync scroll, extract play ID
    let play_id = {
        let mut data = queue.lock();
        let items = match data["items"].as_array() {
            Some(a) if !a.is_empty() => a,
            _ => return,
        };
        let len = items.len();

        if repeat == "one" {
            let id = queue_current_id(&data).map(String::from);
            drop(data);
            if let Some(id) = id {
                handle_playback(shell, audio, state, queue, PlaybackCommand::Play { id });
            }
            return;
        }

        let mut index = data["index"].as_u64().unwrap_or(0) as usize + 1;

        if index >= len {
            if repeat == "all" {
                index = 0;
            } else {
                drop(data);
                audio.stop();
                replace_state(shell, state, default_playback_state());
                return;
            }
        }

        data["index"] = index.into();
        log_err(shell.put(paths::QUEUE_CURRENT, data.clone()), "advance queue");
        queue_current_id(&data).map(String::from)
    };

    if let Some(id) = play_id {
        handle_playback(shell, audio, state, queue, PlaybackCommand::Play { id });
    }
}

fn retreat_queue(shell: &Shell, audio: &dyn AudioBackend, state: &Mutex<Value>, queue: &Mutex<Value>) {
    let play_id = {
        let mut data = queue.lock();
        let items = match data["items"].as_array() {
            Some(a) if !a.is_empty() => a,
            _ => return,
        };
        let len = items.len();
        let index = data["index"].as_u64().unwrap_or(0) as usize;

        let new_index = if index > 0 { index - 1 } else { len - 1 };
        data["index"] = new_index.into();

        log_err(shell.put(paths::QUEUE_CURRENT, data.clone()), "retreat queue");
        queue_current_id(&data).map(String::from)
    };

    if let Some(id) = play_id {
        handle_playback(shell, audio, state, queue, PlaybackCommand::Play { id });
    }
}

pub(crate) fn tick_to_json(outcome: &beeclock_core::TickOutcome) -> Value {
    serde_json::json!({
        "tick": outcome.snapshot.tick,
        "epoch": outcome.snapshot.epoch,
        "partitions": outcome.snapshot.partitions.iter()
            .map(|p| serde_json::json!({
                "name": &p.name,
                "value": p.value,
                "modulus": p.modulus,
            }))
            .collect::<Vec<_>>(),
        "pulses": outcome.pulses.iter()
            .map(|p| &p.name)
            .collect::<Vec<_>>(),
        "overflowed": outcome.overflowed,
    })
}

pub(crate) fn generate_shuffle_order(len: usize, current_index: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).filter(|&i| i != current_index).collect();

    let mut rng_state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42);

    for i in (1..order.len()).rev() {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        let j = (rng_state as usize) % (i + 1);
        order.swap(i, j);
    }

    let mut result = vec![current_index];
    result.extend(order);
    result
}
