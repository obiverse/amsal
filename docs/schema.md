# Amsal Scroll Schema

All state is stored as 9S scrolls. Each scroll has the shape:

```
{ key: String, type_: String, metadata: Metadata, data: Value }
```

Where `Metadata` includes `deleted: Option<bool>` for soft-delete.

---

## Path Conventions

| Prefix | Purpose |
|--------|---------|
| `/amsal/library/{id}` | Media library items |
| `/amsal/art/{id}` | Album art (separate from library to avoid polluting listings) |
| `/amsal/playback/state` | Authoritative playback state |
| `/amsal/playback/command` | Command channel (write to trigger effects) |
| `/amsal/playback/eq` | Equalizer settings |
| `/amsal/queue/current` | Current queue state |
| `/amsal/favorites` | Favorite media IDs |
| `/amsal/playlists/{id}` | Playlists |
| `/amsal/history/{timestamp_ms}` | Play history entries |
| `/amsal/stats/{media_id}` | Per-item play statistics |
| `/amsal/import/request` | Import command channel |
| `/amsal/import/status` | Import status |
| `/amsal/downloads/{id}` | Download state |
| `/amsal/settings/audio` | Audio settings |
| `/amsal/settings/storage` | Storage settings |
| `/amsal/clock/tick` | Latest clock tick snapshot |
| `/amsal/clock/config` | Clock configuration |
| `/amsal/clock/pulses/{name}` | Individual pulse events |

---

## Scroll Schemas

### Library Item — `/amsal/library/{id}`

```json
{
  "id": "song_mp3_abc123",
  "media_type": "Audio",
  "format": "MP3",
  "path": "/absolute/path/to/file.mp3",
  "title": "Song Title",
  "artist": "Artist Name",
  "album": "Album Name",
  "genre": "Rock",
  "duration_ms": 240000
}
```

**Fields:**

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | yes | Stable FNV-1a hash of file path |
| `media_type` | enum | yes | `Audio`, `Video`, `Image`, `Podcast`, `Stream` |
| `format` | enum | yes | `MP3`, `FLAC`, `AAC`, `OGG`, `WAV`, `ALAC`, `OPUS`, `WMA`, `AIFF`, `MP4`, `WEBM`, `MKV`, `PNG`, `JPG`, `WEBP`, or `Other(string)` |
| `path` | string | yes | Absolute filesystem path |
| `title` | string | yes | Extracted from tags or filename |
| `artist` | string | no | From ID3/Vorbis/MP4 tags |
| `album` | string | no | From tags |
| `genre` | string | no | From tags |
| `duration_ms` | u64 | no | Audio duration in milliseconds |

**Deletion:** Soft-delete via `metadata.deleted = true`. `list_library()` filters these out.

---

### Album Art — `/amsal/art/{id}`

```json
{
  "data": "base64-encoded-image-data",
  "mime_type": "image/jpeg"
}
```

| Field | Type | Notes |
|-------|------|-------|
| `data` | string | Base64-encoded image bytes |
| `mime_type` | string | MIME type (e.g. `image/jpeg`, `image/png`) |

Stored under `/amsal/art/` (not `/amsal/library/`) to avoid polluting library listings.

---

### Playback State — `/amsal/playback/state`

```json
{
  "current_id": "song_mp3_abc123",
  "title": "Broken ft Lanjo",
  "artist": "Artist Name",
  "album": "Album Name",
  "playing": true,
  "position_ms": 45000,
  "duration_ms": 240000,
  "volume": 0.8,
  "shuffle": false,
  "repeat": "off"
}
```

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `current_id` | string\|null | null | Currently playing item ID |
| `title` | string | "Unknown" | Track title from library metadata |
| `artist` | string | "Unknown" | Artist name from library metadata |
| `album` | string | "" | Album name from library metadata |
| `playing` | bool | false | Whether audio is playing |
| `position_ms` | u64 | 0 | Current playback position |
| `duration_ms` | u64 | 0 | Total track duration |
| `volume` | f32 | 0.8 | Volume level (0.0 to 1.0) |
| `shuffle` | bool | false | Shuffle mode enabled |
| `repeat` | string | "off" | `"off"`, `"all"`, or `"one"` |
| `error` | string | absent | Set on audio error, cleared on next play |

---

### DSP EQ Chain — `/amsal/playback/eq`

```json
{
  "filters": [
    {"type": "eq", "freq_hz": 80, "gain_db": 3.0, "q": 0.7},
    {"type": "eq", "freq_hz": 3000, "gain_db": -2.0, "q": 1.0},
    {"type": "gain", "db": -1.5}
  ]
}
```

| Filter Type | Fields | Notes |
|-------------|--------|-------|
| `eq` | `freq_hz`, `gain_db`, `q` | Peaking EQ biquad filter |
| `gain` | `db` | Simple gain (positive = boost, negative = cut) |

Write to this path to hot-swap the DSP chain. The engine polls scroll version every 250ms and rebuilds the filter chain on change. Filters are applied in order in the cpal output callback, after volume.

---

### Playback Command — `/amsal/playback/command`

Write to this path to trigger playback effects. Tagged enum with `action` field.

```json
{"action": "play", "id": "song_mp3_abc123"}
{"action": "pause"}
{"action": "resume"}
{"action": "stop"}
{"action": "seek", "position_ms": 120000}
{"action": "next"}
{"action": "previous"}
{"action": "setvolume", "volume": 0.5}
{"action": "setshuffle", "enabled": true}
{"action": "setrepeat", "mode": "all"}
```

---

### Queue State — `/amsal/queue/current`

```json
{
  "items": ["song-a", "song-b", "song-c"],
  "index": 1,
  "shuffle": false,
  "shuffle_order": [1, 2, 0]
}
```

| Field | Type | Notes |
|-------|------|-------|
| `items` | string[] | Media IDs in queue |
| `index` | usize | Current position (into shuffle_order if shuffling) |
| `shuffle` | bool | Shuffle mode active |
| `shuffle_order` | usize[] | Present only when shuffle=true. Maps index to actual item position |

---

### Favorites — `/amsal/favorites`

```json
{
  "ids": ["song-a", "song-b"]
}
```

---

### Playlist — `/amsal/playlists/{id}`

```json
{
  "id": "pl-1",
  "name": "Road Trip",
  "items": ["song-a", "song-b"],
  "created_ms": 1700000000000
}
```

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Playlist identifier |
| `name` | string | Display name |
| `items` | string[] | Ordered list of media IDs |
| `created_ms` | i64 | Creation timestamp (millis since epoch) |

**Deletion:** Soft-delete via `metadata.deleted = true`.

---

### Play History — `/amsal/history/{timestamp_ms}`

```json
{
  "media_id": "song-a",
  "played_at_ms": 1700000000000,
  "duration_played_ms": 180000
}
```

Path key is the timestamp, enabling chronological sort by path.

---

### Media Stats — `/amsal/stats/{media_id}`

```json
{
  "media_id": "song-a",
  "play_count": 42,
  "total_played_ms": 7560000,
  "last_played_ms": 1700000000000
}
```

---

### Import Request — `/amsal/import/request`

```json
{"dir": "/path/to/music"}
```
or
```json
{"file": "/path/to/song.mp3"}
```

### Import Status — `/amsal/import/status`

```json
{
  "scanning": false,
  "imported": 127,
  "dir": "/path/to/music"
}
```

---

### Clock Tick — `/amsal/clock/tick`

```json
{
  "tick": 42,
  "epoch": 1,
  "partitions": [
    {"name": "sub", "value": 2, "modulus": 4},
    {"name": "beat", "value": 1, "modulus": 4},
    {"name": "bar", "value": 0, "modulus": 4}
  ],
  "pulses": ["beat"],
  "overflowed": false
}
```

### Clock Config — `/amsal/clock/config`

```json
{
  "partitions": [
    {"name": "sub", "modulus": 4},
    {"name": "beat", "modulus": 4},
    {"name": "bar", "modulus": 4}
  ],
  "pulses": [
    {"name": "beat", "every": 4},
    {"name": "bar", "every": 16},
    {"name": "phrase", "every": 64}
  ]
}
```

**Validation:** `modulus` and `every` must be > 0. Invalid config falls back to defaults (sub/4, beat/4, bar/4).

### Clock Pulse — `/amsal/clock/pulses/{name}`

```json
{
  "name": "beat",
  "tick": 42,
  "epoch": 1
}
```

---

## Watch Patterns

| Pattern | Matches |
|---------|---------|
| `/amsal/library/**` | All library changes |
| `/amsal/playback/**` | Playback state + commands |
| `/amsal/queue/**` | Queue changes |
| `/amsal/clock/**` | Clock ticks + pulses |
| `/amsal/**` | Everything |

---

## FFI API (v4)

All FFI functions use opaque `EngineHandle*` + C strings + JSON serialization.

- Strings returned by FFI must be freed with `amsal_string_free()`
- NULL return on `*mut c_char` functions indicates error or not-found
- Check `amsal_last_error()` for error details after NULL returns
- `i32` returns: 1 = success, 0 = error

### Functions (36 total)

**Lifecycle:** `amsal_set_root`, `amsal_open`, `amsal_close`, `amsal_version`

**Library:** `amsal_library_add`, `amsal_library_list`, `amsal_delete`, `amsal_search_library`, `amsal_filter_library`

**Scroll I/O:** `amsal_read`, `amsal_write`, `amsal_list`

**Playback:** `amsal_command`, `amsal_playback_state`

**Queue:** `amsal_set_queue`, `amsal_queue_state`

**Import:** `amsal_import_dir`, `amsal_import_file`

**Favorites:** `amsal_set_favorites`, `amsal_get_favorites`

**Album Art:** `amsal_album_art`

**Playlists:** `amsal_create_playlist`, `amsal_get_playlist`, `amsal_list_playlists`, `amsal_add_to_playlist`, `amsal_remove_from_playlist`, `amsal_delete_playlist`, `amsal_rename_playlist`

**History/Stats:** `amsal_play_history`, `amsal_media_stats`, `amsal_top_played`

**Clock:** `amsal_clock_state`, `amsal_configure_clock`

**Error/Memory:** `amsal_last_error`, `amsal_string_free`
