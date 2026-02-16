//! amsal CLI — music player powered by the amsal engine.
//!
//! Commands:
//!   amsal play <file>          Import + play a file
//!   amsal import <dir>         Scan directory, import all media
//!   amsal list                 List library items
//!   amsal search <query>       Search library
//!   amsal now                  Show current track + position
//!   amsal pause                Pause playback
//!   amsal resume               Resume playback
//!   amsal stop                 Stop playback
//!   amsal next                 Next track
//!   amsal prev                 Previous track
//!   amsal seek <seconds>       Seek to position
//!   amsal volume <0-100>       Set volume
//!   amsal queue <id> [id...]   Set queue from library IDs
//!   amsal shuffle <on|off>     Toggle shuffle
//!   amsal repeat <off|all|one> Set repeat mode
//!   amsal history [limit]      Recent play history
//!   amsal stats <id>           Track statistics

use amsal_core::playback::{PlaybackCommand, RepeatMode};
use amsal_core::Engine;
use nine_s_shell::Shell;

fn main() {
    env_logger::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return;
    }

    // 9S root defaults to ~/.amsal
    if std::env::var("NINE_S_ROOT").is_err() {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let root = format!("{}/.amsal", home);
        std::fs::create_dir_all(&root).ok();
        std::env::set_var("NINE_S_ROOT", &root);
    }

    let shell = Shell::open("amsal", &[]).expect("failed to open 9S shell");
    let engine = Engine::new(shell);

    match args[0].as_str() {
        "play" => cmd_play(&engine, &args[1..]),
        "import" => cmd_import(&engine, &args[1..]),
        "list" => cmd_list(&engine),
        "search" => cmd_search(&engine, &args[1..]),
        "now" => cmd_now(&engine),
        "pause" => { engine.command(PlaybackCommand::Pause).ok(); }
        "resume" => { engine.command(PlaybackCommand::Resume).ok(); }
        "stop" => { engine.command(PlaybackCommand::Stop).ok(); }
        "next" => { engine.command(PlaybackCommand::Next).ok(); }
        "prev" => { engine.command(PlaybackCommand::Previous).ok(); }
        "seek" => cmd_seek(&engine, &args[1..]),
        "volume" => cmd_volume(&engine, &args[1..]),
        "queue" => cmd_queue(&engine, &args[1..]),
        "shuffle" => cmd_shuffle(&engine, &args[1..]),
        "repeat" => cmd_repeat(&engine, &args[1..]),
        "history" => cmd_history(&engine, &args[1..]),
        "stats" => cmd_stats(&engine, &args[1..]),
        other => {
            eprintln!("unknown command: {}", other);
            print_usage();
        }
    }

    engine.shutdown();
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_play(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal play <file>");
        return;
    }
    let file = &args[0];

    // Import the file first
    if let Err(e) = engine.import_file(file) {
        eprintln!("import failed: {}", e);
        return;
    }

    // Start engine loops so playback command is processed
    engine.start();

    // Wait briefly for import effect to process
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Find the imported item by file path
    let id = match find_id_by_path(engine, file) {
        Some(id) => id,
        None => {
            eprintln!("could not find imported item for: {}", file);
            return;
        }
    };

    // Set queue and play
    engine.set_queue(vec![id.clone()], 0).ok();
    engine.command(PlaybackCommand::Play { id }).ok();

    // Block showing progress until track ends (Ctrl+C exits via Drop)
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let state = engine.playback_state();
        let playing = state["playing"].as_bool().unwrap_or(false);
        let pos = state["position_ms"].as_u64().unwrap_or(0);
        let dur = state["duration_ms"].as_u64().unwrap_or(0);
        let title = state["title"].as_str().unwrap_or("Unknown");
        let artist = state["artist"].as_str().unwrap_or("Unknown");
        let vol = state["volume"].as_f64().unwrap_or(1.0);

        print_progress(title, artist, pos, dur, (vol * 100.0) as u32);

        if !playing && pos > 0 {
            break; // Track finished
        }
        if !playing && dur == 0 && pos == 0 {
            // Still loading or stopped
            continue;
        }
    }
    println!();
}

fn cmd_import(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal import <dir>");
        return;
    }

    engine.start();

    if let Err(e) = engine.import_dir(&args[0]) {
        eprintln!("import failed: {}", e);
        return;
    }

    // Wait for import to complete
    std::thread::sleep(std::time::Duration::from_secs(2));

    let count = engine.list_library().map(|v| v.len()).unwrap_or(0);
    println!("library: {} items", count);
}

fn cmd_list(engine: &Engine) {
    let paths = engine.list_library().unwrap_or_default();
    if paths.is_empty() {
        println!("library is empty");
        return;
    }
    for path in &paths {
        let id = path.rsplit('/').next().unwrap_or(path);
        if let Ok(Some(scroll)) = engine.shell().get(path) {
            let d = &scroll.data;
            println!(
                "{}  {} — {}",
                id,
                d["title"].as_str().unwrap_or("?"),
                d["artist"].as_str().unwrap_or("?"),
            );
        }
    }
}

fn cmd_search(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal search <query>");
        return;
    }
    let results = engine.search_library(&args.join(" "));
    if results.is_empty() {
        println!("no results");
        return;
    }
    for item in &results {
        println!(
            "{}  {} — {}",
            item["id"].as_str().unwrap_or("?"),
            item["title"].as_str().unwrap_or("?"),
            item["artist"].as_str().unwrap_or("?"),
        );
    }
}

fn cmd_now(engine: &Engine) {
    let state = engine.playback_state();
    let playing = state["playing"].as_bool().unwrap_or(false);
    let pos = state["position_ms"].as_u64().unwrap_or(0);
    let dur = state["duration_ms"].as_u64().unwrap_or(0);
    let title = state["title"].as_str().unwrap_or("Nothing playing");
    let artist = state["artist"].as_str().unwrap_or("");

    if playing {
        println!("{} — {}", title, artist);
        println!("  {} / {}", fmt_time(pos), fmt_time(dur));
    } else {
        println!("stopped");
    }
}

fn cmd_seek(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal seek <seconds>");
        return;
    }
    if let Ok(secs) = args[0].parse::<u64>() {
        engine.command(PlaybackCommand::Seek { position_ms: secs * 1000 }).ok();
    } else {
        eprintln!("invalid seconds: {}", args[0]);
    }
}

fn cmd_volume(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal volume <0-100>");
        return;
    }
    if let Ok(v) = args[0].parse::<u32>() {
        let vol = (v.min(100) as f32) / 100.0;
        engine.command(PlaybackCommand::SetVolume { volume: vol }).ok();
    } else {
        eprintln!("invalid volume: {}", args[0]);
    }
}

fn cmd_queue(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal queue <id> [id...]");
        return;
    }
    engine.set_queue(args.to_vec(), 0).ok();
    println!("queue set: {} items", args.len());
}

fn cmd_shuffle(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal shuffle <on|off>");
        return;
    }
    match args[0].as_str() {
        "on" => { engine.command(PlaybackCommand::SetShuffle { enabled: true }).ok(); }
        "off" => { engine.command(PlaybackCommand::SetShuffle { enabled: false }).ok(); }
        _ => eprintln!("usage: amsal shuffle <on|off>"),
    }
}

fn cmd_repeat(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal repeat <off|all|one>");
        return;
    }
    let mode = match args[0].as_str() {
        "off" => RepeatMode::Off,
        "all" => RepeatMode::All,
        "one" => RepeatMode::One,
        _ => { eprintln!("usage: amsal repeat <off|all|one>"); return; }
    };
    engine.command(PlaybackCommand::SetRepeat { mode }).ok();
}

fn cmd_history(engine: &Engine, args: &[String]) {
    let limit = args.first().and_then(|s| s.parse().ok()).unwrap_or(10);
    let entries = engine.play_history(limit);
    if entries.is_empty() {
        println!("no history");
        return;
    }
    for entry in &entries {
        println!(
            "{}  {} — {}",
            entry["media_id"].as_str().unwrap_or("?"),
            entry["title"].as_str().unwrap_or("?"),
            entry["artist"].as_str().unwrap_or("?"),
        );
    }
}

fn cmd_stats(engine: &Engine, args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: amsal stats <id>");
        return;
    }
    match engine.media_stats(&args[0]) {
        Some(stats) => println!("{}", serde_json::to_string_pretty(&stats).unwrap_or_default()),
        None => println!("no stats for: {}", args[0]),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_id_by_path(engine: &Engine, file: &str) -> Option<String> {
    let abs = std::fs::canonicalize(file).ok()?;
    let abs_str = abs.to_string_lossy();
    let paths = engine.list_library().ok()?;
    for path in &paths {
        if let Ok(Some(scroll)) = engine.shell().get(path) {
            if let Some(p) = scroll.data["path"].as_str() {
                if p == abs_str.as_ref() {
                    return path.rsplit('/').next().map(String::from);
                }
            }
        }
    }
    None
}

fn print_progress(title: &str, artist: &str, pos_ms: u64, dur_ms: u64, vol: u32) {
    let bar_width = 30;
    let filled = if dur_ms > 0 {
        ((pos_ms as f64 / dur_ms as f64) * bar_width as f64) as usize
    } else {
        0
    };
    let empty = bar_width - filled;

    print!(
        "\r  {} -- {}  [{}{}] {} / {}  vol: {}%    ",
        title,
        artist,
        "=".repeat(filled),
        " ".repeat(empty),
        fmt_time(pos_ms),
        fmt_time(dur_ms),
        vol,
    );
    use std::io::Write;
    std::io::stdout().flush().ok();
}

fn fmt_time(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn print_usage() {
    println!("amsal - CLI music player");
    println!();
    println!("usage: amsal <command> [args]");
    println!();
    println!("commands:");
    println!("  play <file>            Import + play a file");
    println!("  import <dir>           Scan directory, import all media");
    println!("  list                   List library items");
    println!("  search <query>         Search library");
    println!("  now                    Show current track + position");
    println!("  pause                  Pause playback");
    println!("  resume                 Resume playback");
    println!("  stop                   Stop playback");
    println!("  next                   Next track");
    println!("  prev                   Previous track");
    println!("  seek <seconds>         Seek to position");
    println!("  volume <0-100>         Set volume");
    println!("  queue <id> [id...]     Set queue from library IDs");
    println!("  shuffle <on|off>       Toggle shuffle");
    println!("  repeat <off|all|one>   Set repeat mode");
    println!("  history [limit]        Recent play history");
    println!("  stats <id>             Track statistics");
}
