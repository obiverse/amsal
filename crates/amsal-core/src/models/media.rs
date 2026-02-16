//! Media classification types.
//!
//! These are string enums — they exist for type-safe matching in Rust,
//! but serialize to plain strings in scrolls. No wrapper structs.

use serde::{Deserialize, Serialize};

/// What kind of media this item represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Audio,
    Video,
    Image,
    Podcast,
    Stream,
}

/// Container/codec format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    MP3,
    FLAC,
    AAC,
    OGG,
    WAV,
    ALAC,
    OPUS,
    WMA,
    AIFF,
    MP4,
    WEBM,
    MKV,
    PNG,
    JPG,
    WEBP,
    Other(String),
}
