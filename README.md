# amsal

A media engine built on the [9S scroll substrate](https://github.com/obiverse/beebank). Everything is a scroll — playback state, queues, playlists, history, and stats are all stored as 9S scrolls with path-based identity.

## Architecture

```
Layer 0: 9S Substrate (paths, scrolls, persistence)
Layer 1: Effects (audio decode/output, file import, clock)
Layer 2: Engine (scroll read/write, command dispatch, heartbeat)
Layer 3: FFI (C API for Flutter, Swift, Kotlin, WASM)
```

**Audio pipeline:** symphonia (decode) → SampleRing (buffer) → cpal (output)

**Supported formats:** MP3, FLAC, AAC, OGG, WAV, ALAC, OPUS, WMA, AIFF, MP4, WEBM, MKV

## Features

- Library management with metadata extraction (ID3, Vorbis, MP4 tags)
- Playback with shuffle, repeat (off/all/one), seek, volume
- Queue management with shuffle ordering
- Playlists (CRUD, soft-delete)
- Album art extraction (base64-encoded)
- Library search and filter
- Play history and per-track statistics
- Configurable clock with partitions and pulses
- Gapless pre-probe for reduced track transition latency
- Channel adaptation (mono↔stereo, up/down-mix)
- 36-function FFI C API (v4)

## Build

```bash
cargo build
```

Requires Rust 1.70+. On Linux, install ALSA dev headers:

```bash
sudo apt-get install libasound2-dev
```

## Test

```bash
cargo test
```

66 tests (51 core + 15 FFI integration).

## FFI Usage

Link against `libamsal_ffi`. All functions use opaque `EngineHandle*` + C strings + JSON serialization.

```c
#include <stdint.h>

// Lifecycle
int32_t amsal_set_root(void* handle, const char* path);
void*   amsal_open(const char* app_name);
void    amsal_close(void* handle);
char*   amsal_version();

// Library
int32_t amsal_library_add(void* handle, const char* path);
char*   amsal_library_list(void* handle);
// ... 32 more functions

// Memory: free strings returned by FFI
void amsal_string_free(char* s);
```

See [docs/schema.md](docs/schema.md) for full scroll schemas and FFI reference.

## License

MIT
