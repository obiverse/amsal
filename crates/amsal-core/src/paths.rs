//! Amsal path conventions over 9S.
//!
//! Pure functions mapping media concepts to scroll paths.
//! No new operations — just reads and writes on agreed-upon paths.

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

pub fn library_path(id: &str) -> String {
    format!("/amsal/library/{}", id)
}

pub const LIBRARY_PREFIX: &str = "/amsal/library";

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

pub const PLAYBACK_STATE: &str = "/amsal/playback/state";
pub const PLAYBACK_COMMAND: &str = "/amsal/playback/command";
pub const PLAYBACK_EQ: &str = "/amsal/playback/eq";

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

pub const QUEUE_CURRENT: &str = "/amsal/queue/current";

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

pub const FAVORITES: &str = "/amsal/favorites";

pub fn playlist_path(id: &str) -> String {
    format!("/amsal/playlists/{}", id)
}

pub const PLAYLISTS_PREFIX: &str = "/amsal/playlists";

// ---------------------------------------------------------------------------
// Album Art (separate prefix to avoid polluting library listings)
// ---------------------------------------------------------------------------

pub fn art_path(id: &str) -> String {
    format!("/amsal/art/{}", id)
}

// ---------------------------------------------------------------------------
// History & Stats
// ---------------------------------------------------------------------------

pub fn history_path(timestamp_ms: i64) -> String {
    format!("/amsal/history/{}", timestamp_ms)
}

pub const HISTORY_PREFIX: &str = "/amsal/history";

pub fn stats_path(media_id: &str) -> String {
    format!("/amsal/stats/{}", media_id)
}

pub const STATS_PREFIX: &str = "/amsal/stats";

// ---------------------------------------------------------------------------
// Import & Downloads
// ---------------------------------------------------------------------------

pub const IMPORT_REQUEST: &str = "/amsal/import/request";
pub const IMPORT_STATUS: &str = "/amsal/import/status";

pub fn download_path(id: &str) -> String {
    format!("/amsal/downloads/{}", id)
}

pub const DOWNLOADS_PREFIX: &str = "/amsal/downloads";

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub const SETTINGS_AUDIO: &str = "/amsal/settings/audio";
pub const SETTINGS_STORAGE: &str = "/amsal/settings/storage";

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

pub const CLOCK_TICK: &str = "/amsal/clock/tick";
pub const CLOCK_CONFIG: &str = "/amsal/clock/config";

pub fn clock_pulse_path(name: &str) -> String {
    format!("/amsal/clock/pulses/{}", name)
}

pub const CLOCK_PULSES_PREFIX: &str = "/amsal/clock/pulses";

// ---------------------------------------------------------------------------
// Watch patterns
// ---------------------------------------------------------------------------

pub const WATCH_LIBRARY: &str = "/amsal/library/**";
pub const WATCH_PLAYBACK: &str = "/amsal/playback/**";
pub const WATCH_QUEUE: &str = "/amsal/queue/**";
pub const WATCH_CLOCK: &str = "/amsal/clock/**";
pub const WATCH_ALL: &str = "/amsal/**";
