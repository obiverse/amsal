//! amsal-core — Media OS kernel over 9S scrolls.
//!
//! All media state is scrolls. Effects are the only code.
//!
//! # Architecture
//!
//! ```text
//! Layer 0: 9S Substrate (paths, scrolls, persistence)
//! Layer 1: Effects (audio decode, import, download)
//! Layer 2: Agents (Flutter, CLI, web — read/write scrolls)
//! Layer 3: Views (UI renders scroll state)
//! ```

pub mod effects;
pub mod engine;
pub mod models;
pub mod paths;

pub use engine::Engine;
pub use models::*;

#[cfg(test)]
mod tests {
    use super::*;
    use nine_s_shell::Shell;
    use once_cell::sync::Lazy;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn temp_engine(app: &str) -> (TempDir, Engine, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = TempDir::new().expect("tempdir");
        std::env::set_var("NINE_S_ROOT", dir.path());
        let shell = Shell::open(app, &[]).expect("shell");
        let engine = Engine::new(shell);
        (dir, engine, guard)
    }

    #[test]
    fn add_and_list_library() {
        let (_dir, engine, _guard) = temp_engine("test-library");

        let data = serde_json::json!({
            "id": "song-001",
            "media_type": "audio",
            "title": "Test Song",
            "artist": "Test Artist",
            "album": "Test Album",
            "genre": "Electronic",
            "duration_ms": 240_000,
            "format": "FLAC",
            "path": "/music/test.flac"
        });

        engine.add_to_library("song-001", data).unwrap();

        let paths = engine.list_library().unwrap();
        assert_eq!(paths, vec!["/amsal/library/song-001"]);

        // Read back
        let scroll = engine
            .shell()
            .get("/amsal/library/song-001")
            .unwrap()
            .unwrap();
        assert_eq!(scroll.data["title"], "Test Song");
        assert_eq!(scroll.data["artist"], "Test Artist");
        assert_eq!(scroll.data["format"], "FLAC");
    }

    #[test]
    fn playback_state_roundtrip() {
        let (_dir, engine, _guard) = temp_engine("test-playback");

        let state = engine.playback_state();
        assert_eq!(state["playing"], false);
        assert_eq!(state["current_id"], serde_json::Value::Null);
    }

    #[test]
    fn favorites_roundtrip() {
        let (_dir, engine, _guard) = temp_engine("test-favorites");

        assert!(engine.favorites().is_empty());

        let ids = vec!["song-001".into(), "song-002".into()];
        engine.set_favorites(&ids).unwrap();

        let loaded = engine.favorites();
        assert_eq!(loaded, ids);
    }

    #[test]
    fn watch_library_changes() {
        let (_dir, engine, _guard) = temp_engine("test-watch");

        let rx = engine.shell().on(paths::WATCH_LIBRARY).unwrap();

        let data = serde_json::json!({
            "id": "song-watch",
            "media_type": "audio",
            "title": "Watch Test",
            "format": "MP3",
            "path": "/music/watch.mp3"
        });

        engine.add_to_library("song-watch", data).unwrap();

        let scroll = rx.recv().unwrap();
        assert_eq!(scroll.key, "/amsal/library/song-watch");
        assert_eq!(scroll.data["title"], "Watch Test");
    }

    #[test]
    fn command_triggers_state_update() {
        let (_dir, engine, _guard) = temp_engine("test-command");

        let data = serde_json::json!({
            "id": "cmd-test",
            "media_type": "audio",
            "title": "Command Test",
            "duration_ms": 180_000,
            "format": "MP3",
            "path": "/nonexistent/test.mp3"
        });
        engine.add_to_library("cmd-test", data).unwrap();

        engine.command(PlaybackCommand::Stop).unwrap();

        let state = engine.playback_state();
        assert_eq!(state["playing"], false);
    }

    #[test]
    fn queue_roundtrip() {
        let (_dir, engine, _guard) = temp_engine("test-queue");

        let queue = engine.queue_state().unwrap();
        assert!(queue["items"].as_array().unwrap().is_empty());
        assert_eq!(queue["index"], 0);

        let items = vec!["a".into(), "b".into(), "c".into()];
        engine.set_queue(items.clone(), 1).unwrap();

        let queue = engine.queue_state().unwrap();
        let stored_items: Vec<String> = queue["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(stored_items, items);
        assert_eq!(queue["index"], 1);
        assert_eq!(
            scroll_ext::queue_current_id(&queue),
            Some("b")
        );
    }

    #[test]
    fn import_request_triggers_status() {
        let (_dir, engine, _guard) = temp_engine("test-import");

        engine.start();

        engine.import_dir("/nonexistent/path").unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));

        let status = engine.shell().get(paths::IMPORT_STATUS).unwrap();
        assert!(status.is_some());
        let data = status.unwrap().data;
        assert_eq!(data["scanning"], false);
        assert_eq!(data["imported"], 0);
    }

    // -------------------------------------------------------------------
    // Shuffle tests
    // -------------------------------------------------------------------

    #[test]
    fn shuffle_order_length_matches() {
        let order = engine::generate_shuffle_order(10, 3);
        assert_eq!(order.len(), 10);
    }

    #[test]
    fn shuffle_order_current_first() {
        let order = engine::generate_shuffle_order(10, 5);
        assert_eq!(order[0], 5);
    }

    #[test]
    fn shuffle_order_all_indices_present() {
        let order = engine::generate_shuffle_order(10, 3);
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn shuffle_order_single_item() {
        let order = engine::generate_shuffle_order(1, 0);
        assert_eq!(order, vec![0]);
    }

    #[test]
    fn set_shuffle_creates_order() {
        let (_dir, engine, _guard) = temp_engine("test-shuffle-enable");
        engine.start();

        let items = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        engine.set_queue(items, 2).unwrap();

        engine
            .command(PlaybackCommand::SetShuffle { enabled: true })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let queue = engine.queue_state().unwrap();
        assert_eq!(queue["shuffle"], true);
        assert!(queue["shuffle_order"].is_array());
        assert_eq!(queue["shuffle_order"].as_array().unwrap().len(), 4);
        assert_eq!(queue["index"], 0);

        engine.shutdown();
    }

    #[test]
    fn set_shuffle_disable_restores_index() {
        let (_dir, engine, _guard) = temp_engine("test-shuffle-disable");
        engine.start();

        let items = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        engine.set_queue(items, 2).unwrap();

        engine
            .command(PlaybackCommand::SetShuffle { enabled: true })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        engine
            .command(PlaybackCommand::SetShuffle { enabled: false })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let queue = engine.queue_state().unwrap();
        assert_eq!(queue["shuffle"], false);
        // shuffle_order should be removed
        assert!(
            queue.get("shuffle_order").is_none()
                || queue["shuffle_order"].is_null()
        );
        // Index resolved back to original position
        assert_eq!(queue["index"], 2);

        engine.shutdown();
    }

    // -------------------------------------------------------------------
    // Queue advance tests
    // -------------------------------------------------------------------

    #[test]
    fn advance_queue_next() {
        let (_dir, engine, _guard) = temp_engine("test-advance-next");
        engine.start();

        for id in &["a", "b", "c"] {
            engine
                .add_to_library(
                    id,
                    serde_json::json!({
                        "id": id, "media_type": "audio", "title": id,
                        "duration_ms": 120000, "format": "MP3",
                        "path": format!("/nonexistent/{}.mp3", id)
                    }),
                )
                .unwrap();
        }

        engine
            .set_queue(vec!["a".into(), "b".into(), "c".into()], 0)
            .unwrap();
        engine.command(PlaybackCommand::Next).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let queue = engine.queue_state().unwrap();
        assert_eq!(queue["index"], 1);

        engine.shutdown();
    }

    #[test]
    fn advance_queue_end_repeat_off_stops() {
        let (_dir, engine, _guard) = temp_engine("test-advance-end-off");
        engine.start();

        for id in &["a", "b"] {
            engine
                .add_to_library(
                    id,
                    serde_json::json!({
                        "id": id, "media_type": "audio", "title": id,
                        "duration_ms": 120000, "format": "MP3",
                        "path": format!("/nonexistent/{}.mp3", id)
                    }),
                )
                .unwrap();
        }

        engine.set_queue(vec!["a".into(), "b".into()], 1).unwrap();
        engine.command(PlaybackCommand::Next).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let state = engine.playback_state();
        assert_eq!(state["playing"], false);

        engine.shutdown();
    }

    #[test]
    fn advance_queue_repeat_all_wraps() {
        let (_dir, engine, _guard) = temp_engine("test-advance-repeat-all");
        engine.start();

        for id in &["a", "b"] {
            engine
                .add_to_library(
                    id,
                    serde_json::json!({
                        "id": id, "media_type": "audio", "title": id,
                        "duration_ms": 120000, "format": "MP3",
                        "path": format!("/nonexistent/{}.mp3", id)
                    }),
                )
                .unwrap();
        }

        engine.set_queue(vec!["a".into(), "b".into()], 1).unwrap();
        engine
            .command(PlaybackCommand::SetRepeat {
                mode: RepeatMode::All,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));

        engine.command(PlaybackCommand::Next).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let queue = engine.queue_state().unwrap();
        assert_eq!(queue["index"], 0); // Wrapped

        engine.shutdown();
    }

    #[test]
    fn advance_queue_repeat_one_replays_current() {
        let (_dir, engine, _guard) = temp_engine("test-advance-repeat-one");
        engine.start();

        for id in &["x", "y", "z"] {
            engine
                .add_to_library(
                    id,
                    serde_json::json!({
                        "id": id, "media_type": "audio", "title": id,
                        "duration_ms": 120000, "format": "MP3",
                        "path": format!("/nonexistent/{}.mp3", id)
                    }),
                )
                .unwrap();
        }

        engine
            .set_queue(vec!["x".into(), "y".into(), "z".into()], 1)
            .unwrap();
        engine
            .command(PlaybackCommand::SetRepeat {
                mode: RepeatMode::One,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));

        engine.command(PlaybackCommand::Next).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let queue = engine.queue_state().unwrap();
        assert_eq!(queue["index"], 1); // Did not advance

        let state = engine.playback_state();
        assert_eq!(state["current_id"], "y"); // Replayed same track

        engine.shutdown();
    }

    // -------------------------------------------------------------------
    // Clock tick test
    // -------------------------------------------------------------------

    #[test]
    fn tick_to_json_structure() {
        use beeclock_core::{ClockSnapshot, PartitionState, PulseFired, TickOutcome};

        let outcome = TickOutcome {
            snapshot: ClockSnapshot {
                tick: 42,
                epoch: 1,
                partitions: vec![
                    PartitionState {
                        name: "sub".into(),
                        value: 2,
                        modulus: 4,
                    },
                    PartitionState {
                        name: "beat".into(),
                        value: 1,
                        modulus: 4,
                    },
                ],
            },
            pulses: vec![PulseFired {
                name: "beat".into(),
                tick: 42,
                epoch: 1,
            }],
            overflowed: false,
        };

        let json = engine::tick_to_json(&outcome);
        assert_eq!(json["tick"], 42);
        assert_eq!(json["epoch"], 1);
        assert_eq!(json["overflowed"], false);

        let parts = json["partitions"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["name"], "sub");
        assert_eq!(parts[0]["value"], 2);

        let pulses = json["pulses"].as_array().unwrap();
        assert_eq!(pulses.len(), 1);
        assert_eq!(pulses[0], "beat");
    }

    // -------------------------------------------------------------------
    // Clock config tests
    // -------------------------------------------------------------------

    #[test]
    fn configure_clock_custom_partitions() {
        let (_dir, engine, _guard) = temp_engine("test-clock-config");
        let config = serde_json::json!({
            "partitions": [
                {"name": "tick", "modulus": 8},
                {"name": "measure", "modulus": 4}
            ],
            "pulses": [
                {"name": "measure", "every": 8}
            ]
        });
        engine.configure_clock(config).unwrap();
        engine.start();
        std::thread::sleep(std::time::Duration::from_millis(600));

        let tick = engine.clock_state().unwrap();
        let parts = tick["partitions"].as_array().unwrap();
        let names: Vec<&str> = parts.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["tick", "measure"]);
        assert_eq!(parts[0]["modulus"], 8);
        assert_eq!(parts[1]["modulus"], 4);

        engine.shutdown();
    }

    #[test]
    fn configure_clock_invalid_falls_back() {
        let (_dir, engine, _guard) = temp_engine("test-clock-invalid");
        let config = serde_json::json!({
            "partitions": [{"name": "bad", "modulus": 0}],
            "pulses": []
        });
        engine.configure_clock(config).unwrap();
        engine.start();
        std::thread::sleep(std::time::Duration::from_millis(600));

        let tick = engine.clock_state().unwrap();
        let parts = tick["partitions"].as_array().unwrap();
        let names: Vec<&str> = parts.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["sub", "beat", "bar"]);

        engine.shutdown();
    }

    #[test]
    fn configure_clock_no_config_uses_default() {
        let (_dir, engine, _guard) = temp_engine("test-clock-no-config");
        engine.start();
        std::thread::sleep(std::time::Duration::from_millis(600));

        let tick = engine.clock_state().unwrap();
        let parts = tick["partitions"].as_array().unwrap();
        let names: Vec<&str> = parts.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["sub", "beat", "bar"]);

        engine.shutdown();
    }

    // -------------------------------------------------------------------
    // Shutdown lifecycle tests
    // -------------------------------------------------------------------

    #[test]
    fn shutdown_completes_without_deadlock() {
        let (_dir, engine, _guard) = temp_engine("test-shutdown");
        engine.start();
        std::thread::sleep(std::time::Duration::from_millis(100));
        engine.shutdown();
        let state = engine.playback_state();
        assert_eq!(state["playing"], false);
    }

    #[test]
    fn shutdown_idempotent() {
        let (_dir, engine, _guard) = temp_engine("test-shutdown-idem");
        engine.start();
        std::thread::sleep(std::time::Duration::from_millis(50));
        engine.shutdown();
        engine.shutdown(); // Second call should be safe
    }

    // -------------------------------------------------------------------
    // Library delete tests
    // -------------------------------------------------------------------

    #[test]
    fn delete_from_library_filters_listing() {
        let (_dir, engine, _guard) = temp_engine("test-delete");

        engine
            .add_to_library(
                "song-a",
                serde_json::json!({
                    "id": "song-a", "media_type": "audio", "title": "Song A",
                    "format": "MP3", "path": "/music/a.mp3"
                }),
            )
            .unwrap();
        engine
            .add_to_library(
                "song-b",
                serde_json::json!({
                    "id": "song-b", "media_type": "audio", "title": "Song B",
                    "format": "MP3", "path": "/music/b.mp3"
                }),
            )
            .unwrap();

        let paths = engine.list_library().unwrap();
        assert_eq!(paths.len(), 2);

        engine.delete_from_library("song-a").unwrap();

        let paths = engine.list_library().unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "/amsal/library/song-b");

        // Scroll still exists (soft delete) but metadata.deleted = true
        let scroll = engine
            .shell()
            .get("/amsal/library/song-a")
            .unwrap()
            .unwrap();
        assert_eq!(scroll.metadata.deleted, Some(true));
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let (_dir, engine, _guard) = temp_engine("test-delete-missing");
        let result = engine.delete_from_library("nonexistent");
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------
    // Gapless pre-probe test
    // -------------------------------------------------------------------

    #[test]
    fn prepare_next_smoke_test() {
        use crate::effects::audio::AudioEffect;
        let audio = AudioEffect::new();
        // Should not panic even with nonexistent file
        audio.prepare_next("/nonexistent/track.mp3");
    }

    // -------------------------------------------------------------------
    // History & stats tests
    // -------------------------------------------------------------------

    #[test]
    fn record_play_updates_stats() {
        let (_dir, engine, _guard) = temp_engine("test-stats");
        engine.record_play("song-a", 180_000);
        engine.record_play("song-a", 180_000);
        engine.record_play("song-b", 120_000);

        let stats = engine.media_stats("song-a").unwrap();
        assert_eq!(stats["play_count"], 2);
        assert_eq!(stats["total_played_ms"], 360_000);
        assert!(stats["last_played_ms"].as_i64().unwrap() > 0);

        let stats_b = engine.media_stats("song-b").unwrap();
        assert_eq!(stats_b["play_count"], 1);
    }

    #[test]
    fn history_entries_created() {
        let (_dir, engine, _guard) = temp_engine("test-history");
        engine.record_play("song-a", 100_000);
        std::thread::sleep(std::time::Duration::from_millis(2));
        engine.record_play("song-b", 200_000);

        let history = engine.play_history(10);
        assert_eq!(history.len(), 2);
        // Most recent first
        assert_eq!(history[0]["media_id"], "song-b");
        assert_eq!(history[1]["media_id"], "song-a");
    }

    #[test]
    fn top_played_ordering() {
        let (_dir, engine, _guard) = temp_engine("test-top-played");
        engine.record_play("song-a", 100_000);
        engine.record_play("song-b", 100_000);
        engine.record_play("song-b", 100_000);
        engine.record_play("song-c", 100_000);
        engine.record_play("song-c", 100_000);
        engine.record_play("song-c", 100_000);

        let top = engine.top_played(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0]["media_id"], "song-c");
        assert_eq!(top[0]["play_count"], 3);
        assert_eq!(top[1]["media_id"], "song-b");
        assert_eq!(top[1]["play_count"], 2);
    }

    // -------------------------------------------------------------------
    // Search & filter tests
    // -------------------------------------------------------------------

    #[test]
    fn search_library_partial_title() {
        let (_dir, engine, _guard) = temp_engine("test-search");
        engine
            .add_to_library(
                "s1",
                serde_json::json!({
                    "id": "s1", "title": "Bohemian Rhapsody", "artist": "Queen",
                    "genre": "Rock", "format": "MP3", "path": "/m/a.mp3"
                }),
            )
            .unwrap();
        engine
            .add_to_library(
                "s2",
                serde_json::json!({
                    "id": "s2", "title": "Stairway to Heaven", "artist": "Led Zeppelin",
                    "genre": "Rock", "format": "MP3", "path": "/m/b.mp3"
                }),
            )
            .unwrap();
        engine
            .add_to_library(
                "s3",
                serde_json::json!({
                    "id": "s3", "title": "Chill Vibes", "artist": "Lo-Fi",
                    "genre": "Electronic", "format": "MP3", "path": "/m/c.mp3"
                }),
            )
            .unwrap();

        let results = engine.search_library("bohemian");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Bohemian Rhapsody");

        let results = engine.search_library("queen");
        assert_eq!(results.len(), 1);

        let results = engine.search_library("STAIRWAY");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn filter_library_by_genre() {
        let (_dir, engine, _guard) = temp_engine("test-filter");
        engine
            .add_to_library(
                "s1",
                serde_json::json!({"id": "s1", "title": "A", "genre": "Rock", "format": "MP3", "path": "/m/a.mp3"}),
            )
            .unwrap();
        engine
            .add_to_library(
                "s2",
                serde_json::json!({"id": "s2", "title": "B", "genre": "Electronic", "format": "MP3", "path": "/m/b.mp3"}),
            )
            .unwrap();
        engine
            .add_to_library(
                "s3",
                serde_json::json!({"id": "s3", "title": "C", "genre": "Rock", "format": "MP3", "path": "/m/c.mp3"}),
            )
            .unwrap();

        let results = engine.filter_library("genre", "Rock");
        assert_eq!(results.len(), 2);

        let results = engine.filter_library("genre", "Electronic");
        assert_eq!(results.len(), 1);
    }

    // -------------------------------------------------------------------
    // Album art tests
    // -------------------------------------------------------------------

    #[test]
    fn art_path_convention() {
        assert_eq!(paths::art_path("song-001"), "/amsal/art/song-001");
    }

    #[test]
    fn album_art_roundtrip() {
        let (_dir, engine, _guard) = temp_engine("test-art");
        // Simulate art being written (as import would do)
        engine
            .shell()
            .put(
                &paths::art_path("song-001"),
                serde_json::json!({"data": "dGVzdA==", "mime_type": "image/png"}),
            )
            .unwrap();

        let art = engine.album_art("song-001").unwrap();
        assert_eq!(art["mime_type"], "image/png");
        assert_eq!(art["data"], "dGVzdA==");
    }

    // -------------------------------------------------------------------
    // Playlist tests
    // -------------------------------------------------------------------

    #[test]
    fn playlist_create_and_list() {
        let (_dir, engine, _guard) = temp_engine("test-playlist-crud");
        engine.create_playlist("pl-1", "Road Trip").unwrap();
        engine.create_playlist("pl-2", "Chill").unwrap();

        let paths = engine.list_playlists();
        assert_eq!(paths.len(), 2);

        let data = engine.playlist("pl-1").unwrap();
        assert_eq!(data["name"], "Road Trip");
        assert!(data["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn playlist_add_remove_items() {
        let (_dir, engine, _guard) = temp_engine("test-playlist-items");
        engine.create_playlist("pl-1", "Mix").unwrap();
        engine.add_to_playlist("pl-1", "song-a").unwrap();
        engine.add_to_playlist("pl-1", "song-b").unwrap();

        let data = engine.playlist("pl-1").unwrap();
        assert_eq!(data["items"].as_array().unwrap().len(), 2);

        engine.remove_from_playlist("pl-1", "song-a").unwrap();
        let data = engine.playlist("pl-1").unwrap();
        let items: Vec<&str> = data["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(items, vec!["song-b"]);
    }

    #[test]
    fn playlist_delete_filters_listing() {
        let (_dir, engine, _guard) = temp_engine("test-playlist-delete");
        engine.create_playlist("pl-1", "Keep").unwrap();
        engine.create_playlist("pl-2", "Delete").unwrap();
        engine.delete_playlist("pl-2").unwrap();

        let paths = engine.list_playlists();
        assert_eq!(paths.len(), 1);
        assert!(engine.playlist("pl-2").is_none());
    }

    #[test]
    fn playlist_rename() {
        let (_dir, engine, _guard) = temp_engine("test-playlist-rename");
        engine.create_playlist("pl-1", "Old Name").unwrap();
        engine.rename_playlist("pl-1", "New Name").unwrap();

        let data = engine.playlist("pl-1").unwrap();
        assert_eq!(data["name"], "New Name");
    }
}
