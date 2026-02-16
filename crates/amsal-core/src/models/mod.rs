//! Amsal data models.
//!
//! Philosophy: the scroll IS the data. These types exist only where
//! Rust type safety genuinely helps — tagged enums for dispatch,
//! string enums for classification. State is plain JSON in scrolls.

pub mod media;
pub mod playback;
pub mod scroll_ext;

pub use media::{Format, MediaType};
pub use playback::{PlaybackCommand, RepeatMode};
pub use scroll_ext::ScrollExt;
