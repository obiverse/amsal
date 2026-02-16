//! C FFI surface for amsal.
//!
//! Pattern: opaque EngineHandle + C strings + JSON serialization.
//! Follows nine-s-ffi conventions from beebank.
//!
//! Flutter/Dart calls these via `dart:ffi`. Any platform with C FFI
//! (Swift, Kotlin, Python, Node.js) can use this.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use amsal_core::Engine;
use nine_s_shell::Shell;

// ---------------------------------------------------------------------------
// Error handling (thread-local last error)
// ---------------------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_error(msg: String) {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = Some(msg));
}

fn clear_error() {
    LAST_ERROR.with(|cell| *cell.borrow_mut() = None);
}

/// Returns the last error message (caller frees with `amsal_string_free`).
#[no_mangle]
pub extern "C" fn amsal_last_error() -> *mut c_char {
    LAST_ERROR.with(|cell| {
        cell.borrow_mut()
            .take()
            .and_then(|s| CString::new(s).ok())
            .map(|s| s.into_raw())
            .unwrap_or(ptr::null_mut())
    })
}

/// Frees a string returned from amsal FFI.
///
/// # Safety
/// Must be a pointer returned from this FFI and not already freed.
#[no_mangle]
pub unsafe extern "C" fn amsal_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}

// ---------------------------------------------------------------------------
// Opaque handle
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct EngineHandle {
    _private: [u8; 0],
}

struct EngineHandleInner {
    engine: Engine,
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Sets the 9S storage root directory.
///
/// # Safety
/// `path` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn amsal_set_root(path: *const c_char) -> i32 {
    clear_error();
    match read_cstr(path) {
        Ok(p) => {
            std::env::set_var("NINE_S_ROOT", p);
            1
        }
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// Opens the amsal engine. Returns an opaque handle.
///
/// # Safety
/// `app_id` must be a valid null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn amsal_open(app_id: *const c_char) -> *mut EngineHandle {
    clear_error();
    let app = match read_cstr(app_id) {
        Ok(a) => a,
        Err(e) => {
            set_error(e);
            return ptr::null_mut();
        }
    };

    match Shell::open(&app, &[]) {
        Ok(shell) => {
            let engine = Engine::new(shell);
            engine.start();
            Box::into_raw(Box::new(EngineHandleInner { engine })) as *mut EngineHandle
        }
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Closes the engine and releases all resources.
#[no_mangle]
pub extern "C" fn amsal_close(handle: *mut EngineHandle) {
    if !handle.is_null() {
        unsafe {
            let inner = Box::from_raw(handle as *mut EngineHandleInner);
            inner.engine.shutdown();
        }
    }
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

/// Add a media item to the library. `id` is the item ID, `json` is the data.
/// Returns the stored scroll JSON (caller frees), or NULL on error.
#[no_mangle]
pub extern "C" fn amsal_library_add(
    handle: *mut EngineHandle,
    id: *const c_char,
    json: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    let id_str = match read_cstr(id) {
        Ok(s) => s,
        Err(e) => return err_null(e),
    };
    let json_str = match read_cstr(json) {
        Ok(s) => s,
        Err(e) => return err_null(e),
    };
    let value: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => return err_null(e.to_string()),
    };

    match engine.add_to_library(&id_str, value) {
        Ok(scroll) => json_to_cstr(&scroll),
        Err(e) => err_null(e.to_string()),
    }
}

/// List all library item paths. Returns JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_library_list(handle: *mut EngineHandle) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    match engine.list_library() {
        Ok(paths) => to_cstr(serde_json::to_string(&paths).unwrap_or_default()),
        Err(e) => err_null(e.to_string()),
    }
}

/// Soft-delete a library item (marks as deleted, still exists in 9S).
/// Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_delete(handle: *mut EngineHandle, id: *const c_char) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let id_str = match read_cstr(id) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    match engine.delete_from_library(&id_str) {
        Ok(_) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

/// Read a scroll at any path. Returns JSON (caller frees), or NULL if not found.
#[no_mangle]
pub extern "C" fn amsal_read(handle: *mut EngineHandle, path: *const c_char) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    let path_str = match read_cstr(path) {
        Ok(s) => s,
        Err(e) => return err_null(e),
    };
    match engine.shell().get(&path_str) {
        Ok(Some(scroll)) => json_to_cstr(&scroll),
        Ok(None) => ptr::null_mut(),
        Err(e) => err_null(e.to_string()),
    }
}

/// Write JSON data to any path. Returns stored scroll JSON (caller frees).
#[no_mangle]
pub extern "C" fn amsal_write(
    handle: *mut EngineHandle,
    path: *const c_char,
    json: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    let path_str = match read_cstr(path) {
        Ok(s) => s,
        Err(e) => return err_null(e),
    };
    let json_str = match read_cstr(json) {
        Ok(s) => s,
        Err(e) => return err_null(e),
    };
    let value: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => return err_null(e.to_string()),
    };
    match engine.shell().put(&path_str, value) {
        Ok(scroll) => json_to_cstr(&scroll),
        Err(e) => err_null(e.to_string()),
    }
}

/// List paths under a prefix. Returns JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_list(handle: *mut EngineHandle, prefix: *const c_char) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    let prefix_str = match read_cstr(prefix) {
        Ok(s) => s,
        Err(e) => return err_null(e),
    };
    match engine.shell().all(&prefix_str) {
        Ok(paths) => to_cstr(serde_json::to_string(&paths).unwrap_or_default()),
        Err(e) => err_null(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Playback commands
// ---------------------------------------------------------------------------

/// Send a playback command. `json` is a PlaybackCommand JSON.
/// Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_command(handle: *mut EngineHandle, json: *const c_char) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let json_str = match read_cstr(json) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let cmd: amsal_core::PlaybackCommand = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            set_error(e.to_string());
            return 0;
        }
    };
    match engine.command(cmd) {
        Ok(_) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

/// Get current playback state as JSON (caller frees).
#[no_mangle]
pub extern "C" fn amsal_playback_state(handle: *mut EngineHandle) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    let state = engine.playback_state();
    to_cstr(serde_json::to_string(&state).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

/// Set the playback queue. `ids_json` is a JSON array of media IDs.
/// Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_set_queue(
    handle: *mut EngineHandle,
    ids_json: *const c_char,
    start_index: u32,
) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let json_str = match read_cstr(ids_json) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let ids: Vec<String> = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            set_error(e.to_string());
            return 0;
        }
    };
    match engine.set_queue(ids, start_index as usize) {
        Ok(_) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

/// Get current queue state as JSON (caller frees).
/// Returns NULL if no queue state exists.
#[no_mangle]
pub extern "C" fn amsal_queue_state(handle: *mut EngineHandle) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    match engine.queue_state() {
        Some(state) => to_cstr(serde_json::to_string(&state).unwrap_or_default()),
        None => ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Request a directory scan for media files.
/// Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_import_dir(handle: *mut EngineHandle, dir: *const c_char) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let dir_str = match read_cstr(dir) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    match engine.import_dir(&dir_str) {
        Ok(_) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

/// Import a single file into the library.
/// Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_import_file(handle: *mut EngineHandle, path: *const c_char) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let path_str = match read_cstr(path) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    match engine.import_file(&path_str) {
        Ok(_) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

// ---------------------------------------------------------------------------
// Favorites
// ---------------------------------------------------------------------------

/// Set favorites. `ids_json` is a JSON array of media IDs.
#[no_mangle]
pub extern "C" fn amsal_set_favorites(
    handle: *mut EngineHandle,
    ids_json: *const c_char,
) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let json_str = match read_cstr(ids_json) {
        Ok(s) => s,
        Err(e) => {
            set_error(e);
            return 0;
        }
    };
    let ids: Vec<String> = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            set_error(e.to_string());
            return 0;
        }
    };
    match engine.set_favorites(&ids) {
        Ok(_) => 1,
        Err(e) => {
            set_error(e.to_string());
            0
        }
    }
}

/// Get favorites as JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_get_favorites(handle: *mut EngineHandle) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    let favs = engine.favorites();
    to_cstr(serde_json::to_string(&favs).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// History & Stats
// ---------------------------------------------------------------------------

/// Get recent play history as JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_play_history(
    handle: *mut EngineHandle,
    limit: u32,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let entries = engine.play_history(limit as usize);
    to_cstr(serde_json::to_string(&entries).unwrap_or_default())
}

/// Get stats for a media item as JSON (caller frees). Returns NULL if no stats.
#[no_mangle]
pub extern "C" fn amsal_media_stats(
    handle: *mut EngineHandle,
    id: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let id_str = match read_cstr(id) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.media_stats(&id_str) {
        Some(data) => to_cstr(serde_json::to_string(&data).unwrap_or_default()),
        None => ptr::null_mut(),
    }
}

/// Get top played items sorted by play count. Returns JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_top_played(
    handle: *mut EngineHandle,
    limit: u32,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let entries = engine.top_played(limit as usize);
    to_cstr(serde_json::to_string(&entries).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Search & Filter
// ---------------------------------------------------------------------------

/// Search library by substring match across title/artist/album/genre.
/// Returns JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_search_library(
    handle: *mut EngineHandle,
    query: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let q = match read_cstr(query) { Ok(s) => s, Err(e) => return err_null(e) };
    let results = engine.search_library(&q);
    to_cstr(serde_json::to_string(&results).unwrap_or_default())
}

/// Filter library by exact match on a field. Returns JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_filter_library(
    handle: *mut EngineHandle,
    field: *const c_char,
    value: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let f = match read_cstr(field) { Ok(s) => s, Err(e) => return err_null(e) };
    let v = match read_cstr(value) { Ok(s) => s, Err(e) => return err_null(e) };
    let results = engine.filter_library(&f, &v);
    to_cstr(serde_json::to_string(&results).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Album Art
// ---------------------------------------------------------------------------

/// Get album art for a library item as JSON (caller frees).
/// Returns NULL if no art found.
#[no_mangle]
pub extern "C" fn amsal_album_art(
    handle: *mut EngineHandle,
    id: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let id_str = match read_cstr(id) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.album_art(&id_str) {
        Some(data) => to_cstr(serde_json::to_string(&data).unwrap_or_default()),
        None => ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

/// Create a new playlist. Returns scroll JSON (caller frees).
#[no_mangle]
pub extern "C" fn amsal_create_playlist(
    handle: *mut EngineHandle,
    id: *const c_char,
    name: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let id_str = match read_cstr(id) { Ok(s) => s, Err(e) => return err_null(e) };
    let name_str = match read_cstr(name) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.create_playlist(&id_str, &name_str) {
        Ok(scroll) => json_to_cstr(&scroll),
        Err(e) => err_null(e.to_string()),
    }
}

/// Get a playlist by ID as JSON (caller frees). Returns NULL if not found.
#[no_mangle]
pub extern "C" fn amsal_get_playlist(
    handle: *mut EngineHandle,
    id: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let id_str = match read_cstr(id) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.playlist(&id_str) {
        Some(data) => to_cstr(serde_json::to_string(&data).unwrap_or_default()),
        None => ptr::null_mut(),
    }
}

/// List all non-deleted playlist paths as JSON array (caller frees).
#[no_mangle]
pub extern "C" fn amsal_list_playlists(handle: *mut EngineHandle) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let paths = engine.list_playlists();
    to_cstr(serde_json::to_string(&paths).unwrap_or_default())
}

/// Add a media item to a playlist. Returns scroll JSON (caller frees).
#[no_mangle]
pub extern "C" fn amsal_add_to_playlist(
    handle: *mut EngineHandle,
    playlist_id: *const c_char,
    media_id: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let pid = match read_cstr(playlist_id) { Ok(s) => s, Err(e) => return err_null(e) };
    let mid = match read_cstr(media_id) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.add_to_playlist(&pid, &mid) {
        Ok(scroll) => json_to_cstr(&scroll),
        Err(e) => err_null(e.to_string()),
    }
}

/// Remove a media item from a playlist. Returns scroll JSON (caller frees).
#[no_mangle]
pub extern "C" fn amsal_remove_from_playlist(
    handle: *mut EngineHandle,
    playlist_id: *const c_char,
    media_id: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let pid = match read_cstr(playlist_id) { Ok(s) => s, Err(e) => return err_null(e) };
    let mid = match read_cstr(media_id) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.remove_from_playlist(&pid, &mid) {
        Ok(scroll) => json_to_cstr(&scroll),
        Err(e) => err_null(e.to_string()),
    }
}

/// Soft-delete a playlist. Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_delete_playlist(
    handle: *mut EngineHandle,
    id: *const c_char,
) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => { set_error(e); return 0; } };
    let id_str = match read_cstr(id) { Ok(s) => s, Err(e) => { set_error(e); return 0; } };
    match engine.delete_playlist(&id_str) {
        Ok(_) => 1,
        Err(e) => { set_error(e.to_string()); 0 }
    }
}

/// Rename a playlist. Returns scroll JSON (caller frees).
#[no_mangle]
pub extern "C" fn amsal_rename_playlist(
    handle: *mut EngineHandle,
    id: *const c_char,
    new_name: *const c_char,
) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => return err_null(e) };
    let id_str = match read_cstr(id) { Ok(s) => s, Err(e) => return err_null(e) };
    let name_str = match read_cstr(new_name) { Ok(s) => s, Err(e) => return err_null(e) };
    match engine.rename_playlist(&id_str, &name_str) {
        Ok(scroll) => json_to_cstr(&scroll),
        Err(e) => err_null(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// Get the latest clock tick state as JSON (caller frees).
/// Returns NULL if the clock hasn't ticked yet.
#[no_mangle]
pub extern "C" fn amsal_clock_state(handle: *mut EngineHandle) -> *mut c_char {
    clear_error();
    let engine = match engine_ref(handle) {
        Ok(e) => e,
        Err(e) => return err_null(e),
    };
    match engine.clock_state() {
        Some(state) => to_cstr(serde_json::to_string(&state).unwrap_or_default()),
        None => ptr::null_mut(),
    }
}

/// Configure the clock. Takes effect on next engine start().
/// Returns 1 on success, 0 on error.
#[no_mangle]
pub extern "C" fn amsal_configure_clock(
    handle: *mut EngineHandle,
    config_json: *const c_char,
) -> i32 {
    clear_error();
    let engine = match engine_ref(handle) { Ok(e) => e, Err(e) => { set_error(e); return 0; } };
    let json_str = match read_cstr(config_json) { Ok(s) => s, Err(e) => { set_error(e); return 0; } };
    let config: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => { set_error(e.to_string()); return 0; }
    };
    match engine.configure_clock(config) {
        Ok(_) => 1,
        Err(e) => { set_error(e.to_string()); 0 }
    }
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

/// Returns the FFI API version.
#[no_mangle]
pub extern "C" fn amsal_version() -> u32 {
    4
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn engine_ref<'a>(handle: *mut EngineHandle) -> Result<&'a Engine, String> {
    if handle.is_null() {
        return Err("null engine handle".into());
    }
    let inner = unsafe { &*(handle as *mut EngineHandleInner) };
    Ok(&inner.engine)
}

fn read_cstr(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Err("null string pointer".into());
    }
    unsafe {
        CStr::from_ptr(ptr)
            .to_str()
            .map(String::from)
            .map_err(|_| "invalid utf-8".into())
    }
}

fn json_to_cstr<T: serde::Serialize>(value: &T) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => to_cstr(json),
        Err(e) => err_null(e.to_string()),
    }
}

fn to_cstr(s: String) -> *mut c_char {
    CString::new(s)
        .map(|c| c.into_raw())
        .unwrap_or(ptr::null_mut())
}

fn err_null(msg: String) -> *mut c_char {
    set_error(msg);
    ptr::null_mut()
}

// ---------------------------------------------------------------------------
// FFI Integration Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::ffi::CString;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    /// Open an engine via FFI in a temp directory. Returns (dir, handle, guard).
    fn ffi_engine(app: &str) -> (TempDir, *mut EngineHandle, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = TempDir::new().expect("tempdir");
        let root = CString::new(dir.path().to_str().unwrap()).unwrap();
        let app_c = CString::new(app).unwrap();
        unsafe {
            amsal_set_root(root.as_ptr());
            let handle = amsal_open(app_c.as_ptr());
            assert!(!handle.is_null(), "amsal_open returned null");
            (dir, handle, guard)
        }
    }

    /// Read a *mut c_char into a String and free it.
    fn read_ffi_string(ptr: *mut c_char) -> String {
        assert!(!ptr.is_null(), "FFI returned null string");
        let s = unsafe { CStr::from_ptr(ptr).to_str().unwrap().to_string() };
        unsafe { amsal_string_free(ptr) };
        s
    }

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    // -------------------------------------------------------------------
    // Lifecycle
    // -------------------------------------------------------------------

    #[test]
    fn ffi_version() {
        assert_eq!(amsal_version(), 4);
    }

    #[test]
    fn ffi_open_close_lifecycle() {
        let (_dir, handle, _guard) = ffi_engine("ffi-lifecycle");
        amsal_close(handle);
    }

    #[test]
    fn ffi_null_handle_returns_error() {
        let ptr = amsal_playback_state(ptr::null_mut());
        assert!(ptr.is_null());
        let err = amsal_last_error();
        if !err.is_null() {
            let msg = read_ffi_string(err);
            assert!(msg.contains("null"));
        }
    }

    // -------------------------------------------------------------------
    // Library CRUD via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_library_add_list_delete() {
        let (_dir, handle, _guard) = ffi_engine("ffi-library");
        let id = c("ffi-song-1");
        let json = c(r#"{"id":"ffi-song-1","title":"FFI Test","format":"MP3","path":"/m/t.mp3"}"#);

        let result = amsal_library_add(handle, id.as_ptr(), json.as_ptr());
        let scroll_json = read_ffi_string(result);
        let scroll: serde_json::Value = serde_json::from_str(&scroll_json).unwrap();
        assert_eq!(scroll["data"]["title"], "FFI Test");

        // List
        let list_ptr = amsal_library_list(handle);
        let list_json = read_ffi_string(list_ptr);
        let paths: Vec<String> = serde_json::from_str(&list_json).unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].contains("ffi-song-1"));

        // Delete
        let del = amsal_delete(handle, id.as_ptr());
        assert_eq!(del, 1);

        // List again — should be empty
        let list_ptr = amsal_library_list(handle);
        let list_json = read_ffi_string(list_ptr);
        let paths: Vec<String> = serde_json::from_str(&list_json).unwrap();
        assert!(paths.is_empty());

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Playback state via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_playback_state_initial() {
        let (_dir, handle, _guard) = ffi_engine("ffi-playback");

        let ptr = amsal_playback_state(handle);
        let json = read_ffi_string(ptr);
        let state: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(state["playing"], false);

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Queue via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_queue_set_and_read() {
        let (_dir, handle, _guard) = ffi_engine("ffi-queue");
        let ids = c(r#"["a","b","c"]"#);

        let ret = amsal_set_queue(handle, ids.as_ptr(), 1);
        assert_eq!(ret, 1);

        let ptr = amsal_queue_state(handle);
        let json = read_ffi_string(ptr);
        let queue: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(queue["index"], 1);
        assert_eq!(queue["items"].as_array().unwrap().len(), 3);

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Favorites via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_favorites_roundtrip() {
        let (_dir, handle, _guard) = ffi_engine("ffi-favs");
        let ids = c(r#"["x","y"]"#);
        let ret = amsal_set_favorites(handle, ids.as_ptr());
        assert_eq!(ret, 1);

        let ptr = amsal_get_favorites(handle);
        let json = read_ffi_string(ptr);
        let favs: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(favs, vec!["x", "y"]);

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Read/Write/List raw scroll via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_read_write_list() {
        let (_dir, handle, _guard) = ffi_engine("ffi-rw");
        let path = c("/amsal/test/raw");
        let data = c(r#"{"hello":"world"}"#);

        let ptr = amsal_write(handle, path.as_ptr(), data.as_ptr());
        let _ = read_ffi_string(ptr); // consume result

        let ptr = amsal_read(handle, path.as_ptr());
        let json = read_ffi_string(ptr);
        let scroll: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(scroll["data"]["hello"], "world");

        let prefix = c("/amsal/test");
        let ptr = amsal_list(handle, prefix.as_ptr());
        let list_json = read_ffi_string(ptr);
        let paths: Vec<String> = serde_json::from_str(&list_json).unwrap();
        assert!(paths.contains(&"/amsal/test/raw".to_string()));

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Search & Filter via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_search_and_filter() {
        let (_dir, handle, _guard) = ffi_engine("ffi-search");

        // Add two items
        let id1 = c("s1");
        let j1 = c(r#"{"id":"s1","title":"Alpha Song","genre":"Rock","format":"MP3","path":"/a.mp3"}"#);
        let _ = read_ffi_string(amsal_library_add(handle, id1.as_ptr(), j1.as_ptr()));

        let id2 = c("s2");
        let j2 = c(r#"{"id":"s2","title":"Beta Track","genre":"Jazz","format":"MP3","path":"/b.mp3"}"#);
        let _ = read_ffi_string(amsal_library_add(handle, id2.as_ptr(), j2.as_ptr()));

        // Search
        let q = c("alpha");
        let ptr = amsal_search_library(handle, q.as_ptr());
        let json = read_ffi_string(ptr);
        let results: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Alpha Song");

        // Filter
        let field = c("genre");
        let val = c("Jazz");
        let ptr = amsal_filter_library(handle, field.as_ptr(), val.as_ptr());
        let json = read_ffi_string(ptr);
        let results: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Beta Track");

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Playlists via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_playlist_crud() {
        let (_dir, handle, _guard) = ffi_engine("ffi-playlist");
        let id = c("pl-1");
        let name = c("Road Trip");

        // Create
        let ptr = amsal_create_playlist(handle, id.as_ptr(), name.as_ptr());
        let _ = read_ffi_string(ptr);

        // Get
        let ptr = amsal_get_playlist(handle, id.as_ptr());
        let json = read_ffi_string(ptr);
        let data: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(data["name"], "Road Trip");

        // Add item
        let mid = c("song-a");
        let ptr = amsal_add_to_playlist(handle, id.as_ptr(), mid.as_ptr());
        let _ = read_ffi_string(ptr);

        // Remove item
        let ptr = amsal_remove_from_playlist(handle, id.as_ptr(), mid.as_ptr());
        let _ = read_ffi_string(ptr);

        // Rename
        let new_name = c("Highway Mix");
        let ptr = amsal_rename_playlist(handle, id.as_ptr(), new_name.as_ptr());
        let _ = read_ffi_string(ptr);

        let ptr = amsal_get_playlist(handle, id.as_ptr());
        let json = read_ffi_string(ptr);
        let data: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(data["name"], "Highway Mix");

        // List
        let ptr = amsal_list_playlists(handle);
        let json = read_ffi_string(ptr);
        let paths: Vec<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(paths.len(), 1);

        // Delete
        let ret = amsal_delete_playlist(handle, id.as_ptr());
        assert_eq!(ret, 1);

        let ptr = amsal_list_playlists(handle);
        let json = read_ffi_string(ptr);
        let paths: Vec<String> = serde_json::from_str(&json).unwrap();
        assert!(paths.is_empty());

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // History & Stats via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_history_and_stats() {
        let (_dir, handle, _guard) = ffi_engine("ffi-history");

        // Record plays via engine (use the core API through the handle)
        let engine = engine_ref(handle).unwrap();
        engine.record_play("song-a", 180_000);
        engine.record_play("song-a", 180_000);

        // Stats via FFI
        let id = c("song-a");
        let ptr = amsal_media_stats(handle, id.as_ptr());
        let json = read_ffi_string(ptr);
        let stats: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(stats["play_count"], 2);

        // History via FFI
        let ptr = amsal_play_history(handle, 10);
        let json = read_ffi_string(ptr);
        let history: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(history.len(), 2);

        // Top played via FFI
        let ptr = amsal_top_played(handle, 5);
        let json = read_ffi_string(ptr);
        let top: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(top[0]["play_count"], 2);

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Album Art via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_album_art() {
        let (_dir, handle, _guard) = ffi_engine("ffi-art");

        // Write art via raw write
        let path = c("/amsal/art/song-1");
        let data = c(r#"{"data":"dGVzdA==","mime_type":"image/png"}"#);
        let _ = read_ffi_string(amsal_write(handle, path.as_ptr(), data.as_ptr()));

        // Read via album_art FFI
        let id = c("song-1");
        let ptr = amsal_album_art(handle, id.as_ptr());
        let json = read_ffi_string(ptr);
        let art: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(art["mime_type"], "image/png");

        // Non-existent art returns null
        let id2 = c("nonexistent");
        let ptr = amsal_album_art(handle, id2.as_ptr());
        assert!(ptr.is_null());

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Clock config via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_clock_config() {
        let (_dir, handle, _guard) = ffi_engine("ffi-clock");
        let config = c(r#"{"partitions":[{"name":"tick","modulus":8}],"pulses":[{"name":"tick","every":8}]}"#);

        let ret = amsal_configure_clock(handle, config.as_ptr());
        assert_eq!(ret, 1);

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // Command via FFI
    // -------------------------------------------------------------------

    #[test]
    fn ffi_command_stop() {
        let (_dir, handle, _guard) = ffi_engine("ffi-cmd");
        let cmd = c(r#"{"action":"stop"}"#);

        let ret = amsal_command(handle, cmd.as_ptr());
        assert_eq!(ret, 1);

        let ptr = amsal_playback_state(handle);
        let json = read_ffi_string(ptr);
        let state: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(state["playing"], false);

        amsal_close(handle);
    }

    // -------------------------------------------------------------------
    // String free safety
    // -------------------------------------------------------------------

    #[test]
    fn ffi_string_free_null_safe() {
        unsafe { amsal_string_free(ptr::null_mut()) };
    }
}
