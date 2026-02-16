//! HTTP media source — stream audio from URLs.
//!
//! Uses symphonia's ReadOnlySource to wrap a non-seekable HTTP response.
//! Feature-gated behind `http` to keep the default build minimal.

use symphonia::core::io::MediaSourceStream;

/// Open an HTTP/HTTPS URL and return a MediaSourceStream for symphonia.
pub fn open_url(url: &str) -> Result<MediaSourceStream, Box<dyn std::error::Error>> {
    let response = ureq::get(url).call()?;
    let reader = response.into_body().into_reader();
    let source = symphonia::core::io::ReadOnlySource::new(reader);
    Ok(MediaSourceStream::new(Box::new(source), Default::default()))
}

/// Extract file extension from a URL, stripping query parameters.
///
/// `"https://example.com/song.mp3?token=abc"` → `Some("mp3")`
pub fn extension_from_url(url: &str) -> Option<String> {
    // Strip query and fragment
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split('#').next().unwrap_or(path);
    // Find last path segment
    let segment = path.rsplit('/').next()?;
    // Extract extension
    let ext = segment.rsplit('.').next()?;
    if ext == segment { return None; } // No dot found
    Some(ext.to_lowercase())
}

/// Check if a path looks like an HTTP URL.
pub fn is_http_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_from_url_strips_query() {
        assert_eq!(
            extension_from_url("https://example.com/song.mp3?token=abc"),
            Some("mp3".into())
        );
    }

    #[test]
    fn extension_from_url_no_query() {
        assert_eq!(
            extension_from_url("https://cdn.example.com/audio/track.flac"),
            Some("flac".into())
        );
    }

    #[test]
    fn extension_from_url_no_extension() {
        assert_eq!(
            extension_from_url("https://example.com/stream"),
            None
        );
    }

    #[test]
    fn is_http_url_checks_scheme() {
        assert!(is_http_url("https://example.com/song.mp3"));
        assert!(is_http_url("http://example.com/song.mp3"));
        assert!(!is_http_url("/home/user/song.mp3"));
        assert!(!is_http_url("song.mp3"));
    }
}
