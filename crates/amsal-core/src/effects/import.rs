//! Import effect — scan directories, extract metadata, write library scrolls.
//!
//! Writes plain JSON to library paths. No MediaItem struct needed.

use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use lofty::prelude::*;
use lofty::probe::Probe;
use nine_s_shell::Shell;

use crate::models::media::{Format, MediaType};

/// Supported audio extensions.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "wav", "opus", "wma", "aiff", "alac",
];

/// Supported video extensions.
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov"];

/// Supported image extensions.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Import a single file into the library. Returns true if imported.
pub fn import_file(shell: &Shell, file_path: &str) -> bool {
    let path = Path::new(file_path);
    if !path.exists() {
        return false;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let media_type = match classify_extension(&ext) {
        Some(t) => t,
        None => return false,
    };
    let format = parse_format(&ext);

    let filename = match path.file_name().and_then(|f| f.to_str()) {
        Some(f) => f,
        None => return false,
    };

    let id = stable_id(file_path, filename);
    let scroll_path = format!("/amsal/library/{}", id);

    // Skip if already imported (dedup on re-scan)
    if let Ok(Some(_)) = shell.get(&scroll_path) {
        return false;
    }

    // Build scroll data as plain JSON
    let mut data = serde_json::json!({
        "id": id,
        "media_type": media_type,
        "format": format,
        "path": file_path,
    });

    // Extract metadata for audio files
    if media_type == MediaType::Audio {
        let (title, artist, album, genre, duration_ms) = extract_audio_metadata(path);
        data["title"] = title.into();
        if let Some(a) = artist {
            data["artist"] = a.into();
        }
        if let Some(a) = album {
            data["album"] = a.into();
        }
        if let Some(g) = genre {
            data["genre"] = g.into();
        }
        if let Some(d) = duration_ms {
            data["duration_ms"] = d.into();
        }
    } else {
        data["title"] = filename.into();
    }

    let ok = shell.put(&scroll_path, data).is_ok();
    if ok && media_type == MediaType::Audio {
        if let Some((b64, mime)) = extract_album_art(path) {
            let art_path = crate::paths::art_path(&id);
            let _ = shell.put(&art_path, serde_json::json!({
                "data": b64,
                "mime_type": mime,
            }));
        }
    }
    ok
}

/// Scan a directory and import all recognized media files. Returns count imported.
pub fn scan_directory(shell: &Shell, dir_path: &str) -> usize {
    scan_directory_inner(shell, dir_path, 0)
}

const MAX_SCAN_DEPTH: usize = 32;

fn scan_directory_inner(shell: &Shell, dir_path: &str, depth: usize) -> usize {
    if depth > MAX_SCAN_DEPTH {
        log::warn!("amsal: scan depth limit reached at {}", dir_path);
        return 0;
    }

    let path = Path::new(dir_path);
    if !path.is_dir() {
        return 0;
    }

    let mut count = 0;

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();

            // Skip directory symlinks to prevent loops
            let is_symlink = std::fs::symlink_metadata(&entry_path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);

            if entry_path.is_file() {
                if let Some(p) = entry_path.to_str() {
                    if import_file(shell, p) {
                        count += 1;
                    }
                }
            } else if entry_path.is_dir() && !is_symlink {
                if let Some(p) = entry_path.to_str() {
                    count += scan_directory_inner(shell, p, depth + 1);
                }
            }
        }
    }

    count
}

fn classify_extension(ext: &str) -> Option<MediaType> {
    if AUDIO_EXTENSIONS.contains(&ext) {
        Some(MediaType::Audio)
    } else if VIDEO_EXTENSIONS.contains(&ext) {
        Some(MediaType::Video)
    } else if IMAGE_EXTENSIONS.contains(&ext) {
        Some(MediaType::Image)
    } else {
        None
    }
}

fn parse_format(ext: &str) -> Format {
    match ext {
        "mp3" => Format::MP3,
        "flac" => Format::FLAC,
        "aac" | "m4a" => Format::AAC,
        "ogg" => Format::OGG,
        "wav" => Format::WAV,
        "alac" => Format::ALAC,
        "opus" => Format::OPUS,
        "wma" => Format::WMA,
        "aiff" => Format::AIFF,
        "mp4" | "mov" | "avi" => Format::MP4,
        "webm" => Format::WEBM,
        "mkv" => Format::MKV,
        "png" => Format::PNG,
        "jpg" | "jpeg" => Format::JPG,
        "webp" => Format::WEBP,
        other => Format::Other(other.to_uppercase()),
    }
}

fn extract_audio_metadata(
    path: &Path,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<u64>,
) {
    let fallback_title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown")
        .to_string();

    let tagged = match Probe::open(path).and_then(|p| p.read()) {
        Ok(t) => t,
        Err(_) => return (fallback_title, None, None, None, None),
    };

    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let props = tagged.properties();

    let title = tag
        .and_then(|t| t.title().map(|s| s.to_string()))
        .unwrap_or(fallback_title);

    let artist = tag.and_then(|t| t.artist().map(|s| s.to_string()));
    let album = tag.and_then(|t| t.album().map(|s| s.to_string()));
    let genre = tag.and_then(|t| t.genre().map(|s| s.to_string()));
    let duration_ms = Some(props.duration().as_millis() as u64).filter(|&d| d > 0);

    (title, artist, album, genre, duration_ms)
}

/// Extract embedded album art from an audio file.
/// Returns (base64_data, mime_type) for the first picture found.
fn extract_album_art(path: &Path) -> Option<(String, String)> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pic = tag.pictures().first()?;
    let mime = pic
        .mime_type()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "image/jpeg".to_string());
    let b64 = STANDARD.encode(pic.data());
    Some((b64, mime))
}

/// Stable ID from file path — FNV-1a hash ensures same file → same ID across scans.
pub(crate) fn stable_id(file_path: &str, filename: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in file_path.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{}_{:016x}", sanitize_id(filename), hash)
}

fn sanitize_id(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::stable_id;

    #[test]
    fn stable_id_deterministic() {
        let a = stable_id("/music/song.mp3", "song.mp3");
        let b = stable_id("/music/song.mp3", "song.mp3");
        assert_eq!(a, b);
    }

    #[test]
    fn stable_id_different_for_different_paths() {
        let a = stable_id("/music/song.mp3", "song.mp3");
        let b = stable_id("/other/song.mp3", "song.mp3");
        assert_ne!(a, b);
    }
}
