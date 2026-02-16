//! Playback commands and repeat mode.
//!
//! PlaybackCommand is a tagged enum — genuinely needed for Rust dispatch.
//! RepeatMode is a string enum.
//! PlaybackState is gone — it's just a JSON scroll at /amsal/playback/state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Repeat mode for queue playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

/// Command written to `/amsal/playback/command` to trigger effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum PlaybackCommand {
    Play { id: String },
    Pause,
    Resume,
    Stop,
    Seek { position_ms: u64 },
    Next,
    Previous,
    SetVolume { volume: f32 },
    SetShuffle { enabled: bool },
    SetRepeat { mode: RepeatMode },
}

impl PlaybackCommand {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_value(v: &Value) -> Option<Self> {
        serde_json::from_value(v.clone()).ok()
    }
}
