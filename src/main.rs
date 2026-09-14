use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;
use clap::{Parser, Subcommand};
use lipuzzles::{
    download_lichess_puzzle_db, BlockCompressedDb, BlockDbBuilder, ColumnarBuilderOptions,
    ColumnarDb, ColumnarDbBuilder, DbBuilder, Puzzle, PuzzleDatabase, QueryCriteria, ThemeMask,
    LPDB_VERSION_V3, LPDB_VERSION_V4,
};

#[derive(Parser)]
#[command(name = "lpdb", author = "Antigravity", version = "0.5.0")]
#[command(about = "High-performance zero-copy chess puzzles database engine", long_about = None)]
struct Cli {
    #[arg(short, long, global = true, default_value = "puzzles.lpdb")]
    db: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Download the latest official Lichess puzzle database (.csv.zst)
    Download {
        /// Destination path for downloaded .csv.zst file
        #[arg(short, long, default_value = "lichess_db_puzzle.csv.zst")]
        output: PathBuf,

        /// Custom download URL
        #[arg(long)]
        url: Option<String>,
    },

    /// Build a packed .lpdb binary from SQLite, CSV.ZST, or direct Lichess download
    Build {
        /// Path to source SQLite database (e.g. lichess_puzzles.db)
        #[arg(long)]
        sqlite: Option<PathBuf>,

        /// Path to source Lichess CSV Zstandard file (e.g. lichess_db_puzzle.csv.zst)
        #[arg(long)]
        csv_zst: Option<PathBuf>,

        /// Automatically download the latest Lichess puzzle database before building
        #[arg(long)]
        download: bool,

        /// Output path for .lpdb binary
        #[arg(short, long, default_value = "puzzles.lpdb")]
        output: PathBuf,

        /// Format version: "v4" (Columnar, ultra-compact <100MB-168MB), "v3" (Block), "v2" (Direct MMap 283MB)
        #[arg(long, default_value = "v4")]
        format: String,

        /// Filter: Minimum Elo rating (e.g. 1000)
        #[arg(long)]
        min_rating: Option<u16>,

        /// Filter: Maximum Elo rating (e.g. 2400)
        #[arg(long)]
        max_rating: Option<u16>,

        /// Filter: Minimum puzzle popularity (-100 to 100, e.g. 85)
        #[arg(long)]
        min_popularity: Option<i8>,

        /// Filter: Minimum number of plays (e.g. 50)
        #[arg(long)]
        min_plays: Option<u32>,

        /// Zstandard compression level for V4 format (1..19, default: 19)
        #[arg(long, default_value_t = 19)]
        compression_level: i32,

        /// Block size in puzzles for V4 format (default: 2048)
        #[arg(long, default_value_t = 2048)]
        block_size: usize,

        /// Limit number of puzzles to import (for testing)
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Inspect database metadata and statistics
    Info,

    /// Lookup a puzzle by ID or index
    Get {
        /// Puzzle ID (e.g. "00008")
        #[arg(long)]
        id: Option<String>,

        /// Puzzle index (0-based)
        #[arg(long)]
        index: Option<usize>,

        /// Render ASCII chessboard
        #[arg(short, long, default_value_t = true)]
        board: bool,
    },

    /// Query puzzles with filters
    Query {
        /// Minimum rating
        #[arg(long)]
        min_rating: Option<u16>,

        /// Maximum rating
        #[arg(long)]
        max_rating: Option<u16>,

        /// Required themes (comma-separated, e.g. "fork,endgame")
        #[arg(long)]
        themes: Option<String>,

        /// Any matching themes (comma-separated, e.g. "mateIn1,mateIn2")
        #[arg(long)]
        any_themes: Option<String>,

        /// Maximum number of puzzles to output
        #[arg(short, long, default_value_t = 10)]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Pick random puzzle(s) matching criteria
    Random {
        /// Minimum rating
        #[arg(long)]
        min_rating: Option<u16>,

        /// Maximum rating
        #[arg(long)]
        max_rating: Option<u16>,

        /// Required themes (comma-separated)
        #[arg(long)]
        themes: Option<String>,

        /// Number of random puzzles to pick
        #[arg(short, long, default_value_t = 1)]
        count: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Benchmark database performance (mmap, random seek, filtering throughput)
    Bench {
        /// Number of random lookups to perform
        #[arg(long, default_value_t = 20_000)]
        iterations: usize,
    },
}

fn print_ascii_board(fen: &str) {
    let board_part = fen.split_whitespace().next().unwrap_or("");
    println!("  +-----------------+");
    let mut rank_num = 8;
    for rank in board_part.split('/') {
        print!("{} |", rank_num);
        for c in rank.chars() {
            if let Some(digit) = c.to_digit(10) {
                for _ in 0..digit {
                    print!(" .");
                }
            } else {
                print!(" {}", c);
            }
        }
        println!(" |");
        rank_num -= 1;
    }
    println!("  +-----------------+");
    println!("    a b c d e f g h");
}

fn print_puzzle(p: &Puzzle, show_board: bool) {
    println!("==================================================");
    println!("Puzzle ID:  {}", p.id);
    println!("Rating:     {}", p.rating);
    println!("Themes:     {}", p.themes.join(", "));
    println!("FEN:        {}", p.fen);
    println!("Moves:      {}", p.moves.join(" "));
    if show_board {
        println!();
        print_ascii_board(&p.fen);
    }
    println!("==================================================");
}

fn detect_db_version(path: &PathBuf) -> u32 {
    if let Ok(mut f) = File::open(path) {
        let mut buf = [0u8; 8];
        if f.read_exact(&mut buf).is_ok() && &buf[..4] == b"LPDB" {
            return u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        }
    }
    4 // Default fallback
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Download { output, url } => {
            download_lichess_puzzle_db(&output, url.as_deref())?;
        }

        Commands::Build {
            sqlite,
            csv_zst,
            download,
            output,
            format,
            min_rating,
            max_rating,
            min_popularity,
            min_plays,
            compression_level,
            block_size,
            limit,
        } => {
            let zst_path = if download {
                let default_zst = PathBuf::from("lichess_db_puzzle.csv.zst");
                if !default_zst.exists() {
                    download_lichess_puzzle_db(&default_zst, None)?;
                }
                Some(default_zst)
            } else {
                csv_zst
            };

            let opts = ColumnarBuilderOptions {
                min_rating,
                max_rating,
                min_popularity,
                min_plays,
                block_size,
                compression_level,
                limit,
            };

            if let Some(sq) = sqlite {
                match format.to_ascii_lowercase().as_str() {
                    "v4" | "columnar" => {
                        println!("Building V4 Columnar Database from SQLite: {:?}", sq);
                        let stats = ColumnarDbBuilder::build_from_sqlite(sq, &output, opts)?;
                        println!(
                            "Done! Imported {} puzzles ({} blocks) -> {:.2} MB",
                            stats.total_puzzles,
                            stats.total_blocks,
                            stats.compressed_size as f64 / (1024.0 * 1024.0)
                        );
                    }
                    "v3" => {
                        println!("Building V3 Block Database from SQLite: {:?}", sq);
                        let stats = BlockDbBuilder::build_from_sqlite(sq, &output, limit)?;
                        println!(
                            "Done! Imported {} puzzles -> {:.2} MB",
                            stats.total_puzzles,
                            stats.compressed_size as f64 / (1024.0 * 1024.0)
                        );
                    }
                    _ => {
                        println!("Building V2 Direct-MMap Database from SQLite: {:?}", sq);
                        let stats = DbBuilder::build_from_sqlite(sq, &output, limit)?;
                        println!(
                            "Done! Imported {} puzzles -> {:.2} MB",
                            stats.total_puzzles,
                            stats.total_bytes_written as f64 / (1024.0 * 1024.0)
                        );
                    }
                }
            } else if let Some(cz) = zst_path {
                println!("Building V4 Columnar Database directly from CSV.ZST: {:?}", cz);
                let stats = ColumnarDbBuilder::build_from_csv_zst(cz, &output, opts)?;
                println!(
                    "Done! Imported {} puzzles ({} blocks) -> {:.2} MB",
                    stats.total_puzzles,
                    stats.total_blocks,
                    stats.compressed_size as f64 / (1024.0 * 1024.0)
                );
            } else {
                eprintln!("Error: Specify either --sqlite <path>, --csv-zst <path>, or --download");
                std::process::exit(1);
            }
        }

        Commands::Info => {
            let ver = detect_db_version(&cli.db);
            let start = Instant::now();
            if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                let load_time = start.elapsed();
                println!("Database:           {:?} (Format V4 - Columnar Block-Compressed)", cli.db);
                println!("Load / MMap Time:   {:.3} ms", load_time.as_secs_f64() * 1000.0);
                println!("Total Puzzles:      {}", db.len());
                println!("Total Blocks:       {} ({} puzzles/block)", db.header().block_count, db.header().block_size);
                println!("Unique Themes:      {}", db.theme_dict().len());
                println!("Rating Range:       {} - {}", db.header().min_rating, db.header().max_rating);
                println!("Zstd Level:         {}", db.header().compression_level);
                println!("File Size:          {:.2} MB", (std::fs::metadata(&cli.db)?.len()) as f64 / (1024.0 * 1024.0));
            } else if ver == LPDB_VERSION_V3 {
                let db = BlockCompressedDb::open(&cli.db)?;
                let load_time = start.elapsed();
                println!("Database:           {:?} (Format V3 - Row Block-Compressed)", cli.db);
                println!("Load / MMap Time:   {:.3} ms", load_time.as_secs_f64() * 1000.0);
                println!("Total Puzzles:      {}", db.len());
                println!("Rating Range:       {} - {}", db.header().min_rating, db.header().max_rating);
                println!("File Size:          {:.2} MB", (std::fs::metadata(&cli.db)?.len()) as f64 / (1024.0 * 1024.0));
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                let load_time = start.elapsed();
                println!("Database:           {:?} (Format V2 - Direct Fixed-Record MMap)", cli.db);
                println!("Load / MMap Time:   {:.3} ms", load_time.as_secs_f64() * 1000.0);
                println!("Total Puzzles:      {}", db.len());
                println!("Rating Range:       {} - {}", db.header().min_rating, db.header().max_rating);
                println!("File Size:          {:.2} MB", (std::fs::metadata(&cli.db)?.len()) as f64 / (1024.0 * 1024.0));
            }
        }

        Commands::Get { id, index, board } => {
            let ver = detect_db_version(&cli.db);
            let puzzle = if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                if let Some(i) = index {
                    db.get(i)
                } else if let Some(id_str) = id {
                    db.find_by_id(&id_str)
                } else {
                    eprintln!("Please specify --id <id> or --index <index>");
                    return Ok(());
                }
            } else if ver == LPDB_VERSION_V3 {
                let db = BlockCompressedDb::open(&cli.db)?;
                if let Some(i) = index {
                    db.get(i)
                } else {
                    eprintln!("Please specify --index <index>");
                    return Ok(());
                }
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                if let Some(i) = index {
                    db.get(i)
                } else if let Some(id_str) = id {
                    db.find_by_id(&id_str)
                } else {
                    eprintln!("Please specify --id <id> or --index <index>");
                    return Ok(());
                }
            };

            match puzzle {
                Some(p) => print_puzzle(&p, board),
                None => println!("Puzzle not found."),
            }
        }

        Commands::Query {
            min_rating,
            max_rating,
            themes,
            any_themes,
            limit,
            json,
        } => {
            let ver = detect_db_version(&cli.db);
            let req_mask = themes.as_deref().map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let any_mask = any_themes.as_deref().map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let criteria = QueryCriteria {
                min_rating,
                max_rating,
                required_themes: req_mask,
                any_themes: any_mask,
            };

            let mut rng = rand::thread_rng();
            let mut results = Vec::new();

            if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                for _ in 0..limit {
                    if let Some(p) = db.random_puzzle(&criteria, &mut rng) {
                        results.push(p);
                    }
                }
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                let matches: Vec<&lipuzzles::PuzzleRecord> = db.filter(&criteria).take(limit).collect();
                for r in matches {
                    let moves = r.get_moves(db.move_pool()).iter().map(|m| m.to_uci()).collect();
                    let t_id = r.theme_id() as usize;
                    let th = if t_id < db.theme_dict().len() {
                        db.theme_dict()[t_id].to_theme_names().into_iter().map(String::from).collect()
                    } else {
                        Vec::new()
                    };
                    results.push(Puzzle {
                        id: r.id_string(),
                        fen: r.fen_string(),
                        moves,
                        rating: r.rating(),
                        themes: th,
                    });
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for p in results {
                    println!("[{}] Rating: {:4} | Themes: {:<35} | Moves: {}", p.id, p.rating, p.themes.join(", "), p.moves.join(" "));
                }
            }
        }

        Commands::Random {
            min_rating,
            max_rating,
            themes,
            count,
            json,
        } => {
            let ver = detect_db_version(&cli.db);
            let req_mask = themes.as_deref().map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let criteria = QueryCriteria {
                min_rating,
                max_rating,
                required_themes: req_mask,
                any_themes: None,
            };

            let mut rng = rand::thread_rng();
            let mut results = Vec::new();

            if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                for _ in 0..count {
                    if let Some(p) = db.random_puzzle(&criteria, &mut rng) {
                        results.push(p);
                    }
                }
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                for _ in 0..count {
                    if let Some(p) = db.random_puzzle(&criteria, &mut rng) {
                        results.push(p);
                    }
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for p in results {
                    print_puzzle(&p, true);
                }
            }
        }

        Commands::Bench { iterations } => {
            let ver = detect_db_version(&cli.db);
            println!("Running benchmarks on {:?} (Version: {})...", cli.db, ver);
            let t0 = Instant::now();

            if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                let open_duration = t0.elapsed();
                println!("V4 Header & Block Index MMap Open: {:.3} ms", open_duration.as_secs_f64() * 1000.0);
                let n = db.len();

                let mut rng = rand::thread_rng();
                let t1 = Instant::now();
                let mut check_sum = 0u64;
                for _ in 0..iterations {
                    let idx = rand::Rng::gen_range(&mut rng, 0..n);
                    if let Some(p) = db.get(idx) {
                        check_sum += p.rating as u64;
                    }
                }
                let rand_duration = t1.elapsed();
                println!(
                    "V4 Columnar Random Seek & Full Decode: {} lookups in {:.3} ms ({:.0} lookups/sec, ~{:.2} µs/lookup) [sum={}]",
                    iterations,
                    rand_duration.as_secs_f64() * 1000.0,
                    iterations as f64 / rand_duration.as_secs_f64(),
                    (rand_duration.as_micros() as f64) / (iterations as f64),
                    check_sum
                );
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                let open_duration = t0.elapsed();
                println!("V2 Zero-Copy MMap Open: {:.3} ms", open_duration.as_secs_f64() * 1000.0);
                let n = db.len();

                let mut rng = rand::thread_rng();
                let t1 = Instant::now();
                let mut check_sum = 0u64;
                for _ in 0..iterations {
                    let idx = rand::Rng::gen_range(&mut rng, 0..n);
                    if let Some(p) = db.get(idx) {
                        check_sum += p.rating as u64;
                    }
                }
                let rand_duration = t1.elapsed();
                println!(
                    "V2 Direct Random Access & Full Decode: {} lookups in {:.3} ms ({:.0} lookups/sec, ~{:.2} ns/lookup) [sum={}]",
                    iterations,
                    rand_duration.as_secs_f64() * 1000.0,
                    iterations as f64 / rand_duration.as_secs_f64(),
                    (rand_duration.as_nanos() as f64) / (iterations as f64),
                    check_sum
                );
            }
        }
    }

    Ok(())
}
