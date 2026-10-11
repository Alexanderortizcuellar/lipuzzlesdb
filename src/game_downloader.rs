//! Downloader module for fetching real Lichess games by puzzle ID or game ID/URL,
//! parsing their moves, and packing them into an ultra-compressed `.lgdb` database.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use serde_json::Value;

use crate::game_db::{LgdbBuilder, LgdbStats};
use crate::san::parse_pgn_moves;

pub struct GameDownloadOptions {
    /// Puzzle IDs to download
    pub puzzle_ids: Vec<String>,
    /// Game IDs or URLs to download
    pub game_ids: Vec<String>,
    /// Optional limit on total games
    pub limit: Option<usize>,
    /// Output file path (e.g. "games.lgdb")
    pub output_path: String,
    /// Zstandard compression level (default 19)
    pub compression_level: i32,
    /// Custom Lichess API Token (otherwise auto-detected)
    pub token: Option<String>,
    /// Pause every N games (0 = disabled)
    pub pause_every: usize,
    /// Pause duration in seconds
    pub pause_duration_secs: f64,
    /// Emit real-time progress as JSON lines to stdout
    pub json_progress: bool,
}

impl Default for GameDownloadOptions {
    fn default() -> Self {
        Self {
            puzzle_ids: Vec::new(),
            game_ids: Vec::new(),
            limit: None,
            output_path: "games.lgdb".to_string(),
            compression_level: 19,
            token: None,
            pause_every: 0,
            pause_duration_secs: 2.0,
            json_progress: false,
        }
    }
}

/// Attempts to load a Lichess token from environment variable or local config.
pub fn find_lichess_token() -> Option<String> {
    if let Ok(tok) = std::env::var("LICHESS_TOKEN") {
        let trimmed = tok.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    let env_paths = [
        r"C:\Users\ASUS\programming\qt_programs\chess\qchess\.env",
        r".env",
    ];

    for path in &env_paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() && (trimmed.starts_with("lip_") || trimmed.contains("lip_")) {
                let token = if let Some(idx) = trimmed.find("lip_") {
                    trimmed[idx..]
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                } else {
                    trimmed
                };
                return Some(token);
            }
        }
    }

    None
}

/// Cleans a Lichess game URL or ID (e.g. "https://lichess.org/KnvUoTQB#49" -> "KnvUoTQB").
pub fn extract_clean_game_id(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.contains("lichess.org/") {
        let segs: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        let last = segs.last().unwrap_or(&trimmed);
        let id_part = last.split('#').next().unwrap_or(last);
        if id_part.len() > 8 {
            id_part[..8].to_string()
        } else {
            id_part.to_string()
        }
    } else {
        let id_part = trimmed.split('#').next().unwrap_or(trimmed);
        if id_part.len() > 8 {
            id_part[..8].to_string()
        } else {
            id_part.to_string()
        }
    }
}

/// Reads IDs (one per line, or comma-separated) from a file or string.
pub fn parse_id_list(input: &str) -> Vec<String> {
    if Path::new(input).exists() {
        if let Ok(file) = File::open(input) {
            let reader = BufReader::new(file);
            let mut ids = Vec::new();
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    for item in trimmed.split(',') {
                        let id = item.trim();
                        if !id.is_empty() {
                            ids.push(id.to_string());
                        }
                    }
                }
            }
            return ids;
        }
    }

    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub struct DownloadProgressStats {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub duration: Duration,
    pub db_stats: LgdbStats,
}

/// Downloads games from Lichess based on puzzle IDs or game IDs, parses moves,
/// and writes the compressed `.lgdb` database file.
pub fn download_and_build_lgdb(options: GameDownloadOptions) -> io::Result<DownloadProgressStats> {
    let t0 = Instant::now();
    let token = options.token.or_else(find_lichess_token);

    if let Some(ref tok) = token {
        let masked = if tok.len() > 8 { &tok[..8] } else { tok };
        println!("Authenticated with Lichess token: {}...", masked);
    } else {
        println!("No Lichess token detected; downloading unauthenticated (rate-limited).");
    }

    enum TargetKind {
        Puzzle(String),
        Game(String),
    }

    let mut targets = Vec::new();
    for p in options.puzzle_ids {
        targets.push(TargetKind::Puzzle(p));
    }
    for g in options.game_ids {
        targets.push(TargetKind::Game(extract_clean_game_id(&g)));
    }

    if let Some(limit) = options.limit {
        targets.truncate(limit);
    }

    if targets.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No puzzle or game IDs specified to download",
        ));
    }

    let total = targets.len();
    if !options.json_progress {
        println!("Queued {} games to download...", total);
    }

    let pb = if !options.json_progress {
        let p = ProgressBar::new(total as u64);
        p.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        Some(p)
    } else {
        None
    };

    let mut builder = LgdbBuilder::new();
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (idx, target) in targets.iter().enumerate() {
        if let Some(ref p) = pb {
            p.set_position(idx as u64);
        }

        let mut current_puzzle_id = String::new();
        let mut current_game_id = String::new();

        match target {
            TargetKind::Puzzle(pid) => {
                current_puzzle_id = pid.clone();
                if let Some(ref p) = pb {
                    p.set_message(format!("Puzzle #{}", pid));
                }
                let url = format!("https://lichess.org/api/puzzle/{}", pid);
                let mut req = ureq::get(&url).set(
                    "User-Agent",
                    "lipuzzlesdb/0.1.0 (contact: user@example.com)",
                );
                if let Some(ref tok) = token {
                    req = req.set("Authorization", &format!("Bearer {}", tok));
                }

                match req.call() {
                    Ok(resp) => {
                        let json_val: Result<Value, _> =
                            serde_json::from_reader(resp.into_reader());
                        match json_val {
                            Ok(val) => {
                                let game = &val["game"];
                                let game_id = game["id"].as_str().unwrap_or("");
                                current_game_id = game_id.to_string();
                                let pgn = game["pgn"].as_str().unwrap_or("");

                                let mut full_game_downloaded = false;
                                if !game_id.is_empty() {
                                    let export_url = format!("https://lichess.org/game/export/{}?moves=true&tags=true&clocks=false&evals=false", game_id);
                                    let mut exp_req = ureq::get(&export_url)
                                        .set("Accept", "application/x-chess-pgn")
                                        .set(
                                            "User-Agent",
                                            "lipuzzlesdb/0.1.0 (contact: user@example.com)",
                                        );
                                    if let Some(ref tok) = token {
                                        exp_req = exp_req
                                            .set("Authorization", &format!("Bearer {}", tok));
                                    }
                                    match exp_req.call() {
                                        Ok(exp_resp) => match exp_resp.into_string() {
                                            Ok(pgn_text) => {
                                                let mut date_str = None;
                                                let mut res_str = "*";
                                                for line in pgn_text.lines() {
                                                    let trimmed = line.trim();
                                                    if trimmed.starts_with("[Date ")
                                                        || trimmed.starts_with("[UTCDate ")
                                                    {
                                                        if let Some(val) = trimmed.split('"').nth(1)
                                                        {
                                                            date_str = Some(val.to_string());
                                                        }
                                                    } else if trimmed.starts_with("[Result ") {
                                                        if let Some(val) = trimmed.split('"').nth(1)
                                                        {
                                                            res_str = val;
                                                        }
                                                    }
                                                }
                                                let (moves, res_opt) = parse_pgn_moves(&pgn_text);
                                                let final_res = res_opt.unwrap_or(res_str);
                                                if !moves.is_empty() {
                                                    builder.add_game(
                                                        pid,
                                                        game_id,
                                                        date_str.as_deref(),
                                                        final_res,
                                                        &moves,
                                                    );
                                                    succeeded += 1;
                                                    full_game_downloaded = true;
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!("Failed to read PGN string: {}", e);
                                            }
                                        },
                                        Err(e) => {
                                            eprintln!(
                                                "Export request failed for game {}: {}",
                                                game_id, e
                                            );
                                        }
                                    }
                                }

                                if !full_game_downloaded {
                                    let (moves, res_opt) = parse_pgn_moves(pgn);
                                    let result_str = if let Some(r) = res_opt {
                                        r
                                    } else if let Some(winner) = game["winner"].as_str() {
                                        if winner == "white" {
                                            "1-0"
                                        } else if winner == "black" {
                                            "0-1"
                                        } else {
                                            "1/2-1/2"
                                        }
                                    } else {
                                        "*"
                                    };

                                    if !moves.is_empty() {
                                        builder.add_game(pid, game_id, None, result_str, &moves);
                                        succeeded += 1;
                                    } else {
                                        failed += 1;
                                    }
                                }
                            }
                            Err(_) => failed += 1,
                        }
                    }
                    Err(_) => failed += 1,
                }
            }

            TargetKind::Game(gid) => {
                current_game_id = gid.clone();
                if let Some(ref p) = pb {
                    p.set_message(format!("Game {}", gid));
                }
                let url = format!("https://lichess.org/game/export/{}?moves=true&tags=true&clocks=false&evals=false", gid);
                let mut req = ureq::get(&url)
                    .set("Accept", "application/x-chess-pgn")
                    .set(
                        "User-Agent",
                        "lipuzzlesdb/0.1.0 (contact: user@example.com)",
                    );
                if let Some(ref tok) = token {
                    req = req.set("Authorization", &format!("Bearer {}", tok));
                }

                match req.call() {
                    Ok(resp) => {
                        if let Ok(pgn_text) = resp.into_string() {
                            let mut date_str = None;
                            let mut result_str = "*";

                            for line in pgn_text.lines() {
                                let trimmed = line.trim();
                                if trimmed.starts_with("[Date ") || trimmed.starts_with("[UTCDate ")
                                {
                                    if let Some(val) = trimmed.split('"').nth(1) {
                                        date_str = Some(val.to_string());
                                    }
                                } else if trimmed.starts_with("[Result ") {
                                    if let Some(val) = trimmed.split('"').nth(1) {
                                        result_str = match val {
                                            "1-0" => "1-0",
                                            "0-1" => "0-1",
                                            "1/2-1/2" => "1/2-1/2",
                                            _ => "*",
                                        };
                                    }
                                }
                            }

                            let (moves, res_opt) = parse_pgn_moves(&pgn_text);
                            let final_res = res_opt.unwrap_or(result_str);

                            if !moves.is_empty() {
                                builder.add_game("", gid, date_str.as_deref(), final_res, &moves);
                                succeeded += 1;
                            } else {
                                failed += 1;
                            }
                        } else {
                            failed += 1;
                        }
                    }
                    Err(_) => failed += 1,
                }
            }
        }

        if options.json_progress {
            let info = serde_json::json!({
                "type": "progress",
                "index": idx + 1,
                "total": total,
                "succeeded": succeeded,
                "failed": failed,
                "total_plies": builder.total_plies(),
                "puzzle_id": current_puzzle_id,
                "game_id": current_game_id,
                "elapsed_sec": t0.elapsed().as_secs_f64(),
            });
            println!("{}", info);
            let _ = std::io::stdout().flush();
        }

        // Polite sleep to prevent HTTP 429 rate limits
        if token.is_some() {
            sleep(Duration::from_millis(50));
        } else {
            sleep(Duration::from_millis(150));
        }

        // Periodic batch cooldown pause
        if options.pause_every > 0 && (idx + 1) % options.pause_every == 0 && idx + 1 < total {
            if options.json_progress {
                let pause_msg = serde_json::json!({
                    "type": "cooldown",
                    "duration_sec": options.pause_duration_secs,
                    "message": format!("Cooling down for {:.1}s after {} games...", options.pause_duration_secs, options.pause_every),
                });
                println!("{}", pause_msg);
                let _ = std::io::stdout().flush();
            } else if let Some(ref p) = pb {
                p.set_message(format!(
                    "Pausing {:.1}s cooldown...",
                    options.pause_duration_secs
                ));
            }
            sleep(Duration::from_secs_f64(options.pause_duration_secs));
        }
    }

    if let Some(ref p) = pb {
        p.finish_with_message("Download complete");
    }

    if !options.json_progress {
        println!(
            "Writing compressed game database to '{}'...",
            options.output_path
        );
    }
    let db_stats = builder.write_to_file(&options.output_path, options.compression_level)?;

    let dur = t0.elapsed();
    if options.json_progress {
        let done_msg = serde_json::json!({
            "type": "complete",
            "attempted": total,
            "succeeded": succeeded,
            "failed": failed,
            "total_plies": db_stats.total_plies,
            "file_size": db_stats.total_file_size,
            "duration_sec": dur.as_secs_f64(),
        });
        println!("{}", done_msg);
        let _ = std::io::stdout().flush();
    }
    Ok(DownloadProgressStats {
        attempted: total,
        succeeded,
        failed,
        duration: dur,
        db_stats,
    })
}
