//! Scroll field accessors — thin helpers over raw JSON.
//!
//! Instead of MediaItem/PlaybackState/QueueState structs with
//! to_value()/from_value() round-trips, we access scroll data directly.
//! The scroll IS the file. These are just typed lenses.

use serde_json::Value;

/// Extension trait for accessing typed fields on scroll data (serde_json::Value).
pub trait ScrollExt {
    fn str_field(&self, key: &str) -> Option<&str>;
    fn u64_field(&self, key: &str) -> u64;
    fn f32_field(&self, key: &str) -> f32;
    fn bool_field(&self, key: &str) -> bool;
    fn str_array(&self, key: &str) -> Vec<&str>;
    fn usize_field(&self, key: &str) -> usize;
}

impl ScrollExt for Value {
    fn str_field(&self, key: &str) -> Option<&str> {
        self[key].as_str()
    }

    fn u64_field(&self, key: &str) -> u64 {
        self[key].as_u64().unwrap_or(0)
    }

    fn f32_field(&self, key: &str) -> f32 {
        self[key].as_f64().unwrap_or(0.0) as f32
    }

    fn bool_field(&self, key: &str) -> bool {
        self[key].as_bool().unwrap_or(false)
    }

    fn str_array(&self, key: &str) -> Vec<&str> {
        self[key]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default()
    }

    fn usize_field(&self, key: &str) -> usize {
        self[key].as_u64().unwrap_or(0) as usize
    }
}

/// Default playback state as raw JSON.
pub fn default_playback_state() -> Value {
    serde_json::json!({
        "playing": false,
        "position_ms": 0,
        "duration_ms": 0,
        "volume": 0.8,
        "shuffle": false,
        "repeat": "off"
    })
}

/// Default queue state as raw JSON.
pub fn default_queue_state() -> Value {
    serde_json::json!({
        "items": [],
        "index": 0,
        "shuffle": false
    })
}

/// Get the current item ID from a queue scroll's data.
pub fn queue_current_id(data: &Value) -> Option<&str> {
    let items = data["items"].as_array()?;
    if items.is_empty() {
        return None;
    }
    let index = data["index"].as_u64().unwrap_or(0) as usize;

    if data["shuffle"].as_bool().unwrap_or(false) {
        // Dereference through shuffle_order
        let order = data["shuffle_order"].as_array()?;
        let effective = order.get(index)?.as_u64()? as usize;
        items.get(effective)?.as_str()
    } else {
        items.get(index)?.as_str()
    }
}

/// Get repeat mode string, defaulting to "off".
pub fn repeat_mode(data: &Value) -> &str {
    data["repeat"].as_str().unwrap_or("off")
}
