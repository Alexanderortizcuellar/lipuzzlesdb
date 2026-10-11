use clap::{Parser, Subcommand};
use lipuzzles::{
    download_and_build_lgdb, download_lichess_puzzle_db, parse_id_list, BlockCompressedDb,
    BlockDbBuilder, ColumnarBuilderOptions, ColumnarDb, ColumnarDbBuilder, DbBuilder, Exporter,
    GameDownloadOptions, Lgdb, Puzzle, PuzzleDatabase, QueryCriteria, ThemeMask, LPDB_VERSION_V3,
    LPDB_VERSION_V4,
};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "lpdb", author = "Antigravity", version = "0.6.0")]
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

        /// Output puzzle as JSON
        #[arg(long)]
        json: bool,
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

        /// Exclude themes (comma-separated, e.g. "mate,mateIn1,mateIn2,backRankMate")
        #[arg(long)]
        exclude_themes: Option<String>,

        /// Exclude the opponent's initial setup move (advances FEN to puzzle start position)
        #[arg(long)]
        exclude_setup_move: bool,

        /// Maximum number of puzzles to output
        #[arg(short, long, default_value_t = 10)]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Output as PGN format
        #[arg(long)]
        pgn: bool,

        /// Output as CSV format
        #[arg(long)]
        csv: bool,
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

        /// Exclude themes (comma-separated)
        #[arg(long)]
        exclude_themes: Option<String>,

        /// Exclude the opponent's initial setup move (advances FEN to puzzle start position)
        #[arg(long)]
        exclude_setup_move: bool,

        /// Number of random puzzles to pick
        #[arg(short, long, default_value_t = 1)]
        count: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Output as PGN format
        #[arg(long)]
        pgn: bool,
    },

    /// Export filtered puzzles to a PGN or CSV file
    Export {
        /// Output destination file path (e.g. "puzzles.pgn" or "puzzles.csv")
        #[arg(short, long)]
        output: PathBuf,

        /// Export format: "pgn" or "csv" (defaults to inferring from file extension)
        #[arg(short, long)]
        format: Option<String>,

        /// Minimum rating
        #[arg(long)]
        min_rating: Option<u16>,

        /// Maximum rating
        #[arg(long)]
        max_rating: Option<u16>,

        /// Required themes (comma-separated, e.g. "hangingPiece", "fork,endgame")
        #[arg(long)]
        themes: Option<String>,

        /// Any matching themes (comma-separated, e.g. "mateIn1,mateIn2")
        #[arg(long)]
        any_themes: Option<String>,

        /// Exclude themes (comma-separated, e.g. "mate,mateIn1,mateIn2")
        #[arg(long)]
        exclude_themes: Option<String>,

        /// Exclude the opponent's initial setup move (advances FEN to puzzle start position)
        #[arg(long)]
        exclude_setup_move: bool,

        /// Sample uniformly across the rating range instead of taking only the lowest rating
        #[arg(long)]
        sample: bool,

        /// Maximum number of puzzles to export
        #[arg(short, long)]
        limit: Option<usize>,
    },

    /// Benchmark database performance (mmap, random seek, filtering throughput)
    Bench {
        /// Number of random lookups to perform
        #[arg(long, default_value_t = 20_000)]
        iterations: usize,
    },

    /// Download games from Lichess associated with puzzles or game IDs into compressed .lgdb format
    FetchGames {
        /// Comma-separated list of puzzle IDs or path to file containing puzzle IDs
        #[arg(short, long)]
        puzzles: Option<String>,

        /// Comma-separated list of game IDs/URLs or path to file containing game IDs
        #[arg(long)]
        game_ids: Option<String>,

        /// Extract puzzle IDs from an existing .lpdb puzzle database
        #[arg(long)]
        from_db: Option<PathBuf>,

        /// Filter by min rating when reading from --from-db
        #[arg(long)]
        min_rating: Option<u16>,

        /// Filter by max rating when reading from --from-db
        #[arg(long)]
        max_rating: Option<u16>,

        /// Filter by themes when reading from --from-db
        #[arg(long)]
        themes: Option<String>,

        /// Randomly sample puzzles when reading from --from-db
        #[arg(long)]
        random: bool,

        /// Maximum number of games to download
        #[arg(short, long)]
        limit: Option<usize>,

        /// Output path for compressed game database (.lgdb)
        #[arg(short, long, default_value = "games.lgdb")]
        output: PathBuf,

        /// Zstandard compression level (1..19, default: 19)
        #[arg(long, default_value_t = 19)]
        compression_level: i32,

        /// Lichess API personal access token (auto-detected if omitted)
        #[arg(long)]
        token: Option<String>,

        /// Skip confirmation prompt for large downloads (>1000 games)
        #[arg(short = 'y', long)]
        yes: bool,

        /// Pause every N games to allow rate limit cooldown (0 = disabled)
        #[arg(long, default_value_t = 0)]
        pause_every: usize,

        /// Duration of cooldown pause in seconds
        #[arg(long, default_value_t = 2.0)]
        pause_seconds: f64,

        /// Output progress as JSON lines to stdout
        #[arg(long)]
        json_progress: bool,
    },

    /// Inspect a compressed .lgdb game database or export games to PGN
    InspectGames {
        /// Path to .lgdb game database file
        #[arg(short, long, default_value = "games.lgdb")]
        games: PathBuf,

        /// Number of sample games to preview
        #[arg(short, long, default_value_t = 5)]
        limit: usize,

        /// Specific game index to fetch (0-based)
        #[arg(long)]
        index: Option<usize>,

        /// Lookup game by associated Puzzle ID
        #[arg(long)]
        puzzle_id: Option<String>,

        /// Lookup game by Lichess Game ID
        #[arg(long)]
        game_id: Option<String>,

        /// Output results as JSON
        #[arg(long)]
        json: bool,

        /// Export all games from the .lgdb database into a PGN file
        #[arg(long)]
        export_pgn: Option<PathBuf>,
    },
}

fn piece_to_unicode(c: char) -> char {
    match c {
        // White pieces
        'K' => '♔',
        'Q' => '♕',
        'R' => '♖',
        'B' => '♗',
        'N' => '♘',
        'P' => '♙',
        // Black pieces
        'k' => '♚',
        'q' => '♛',
        'r' => '♜',
        'b' => '♝',
        'n' => '♞',
        'p' => '♟',
        _ => c,
    }
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
                    print!(" ·");
                }
            } else {
                print!(" {}", piece_to_unicode(c));
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
                println!(
                    "Building V4 Columnar Database directly from CSV.ZST: {:?}",
                    cz
                );
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
                println!(
                    "Database:           {:?} (Format V4 - Columnar Block-Compressed)",
                    cli.db
                );
                println!(
                    "Load / MMap Time:   {:.3} ms",
                    load_time.as_secs_f64() * 1000.0
                );
                println!("Total Puzzles:      {}", db.len());
                println!(
                    "Total Blocks:       {} ({} puzzles/block)",
                    db.header().block_count,
                    db.header().block_size
                );
                println!("Unique Themes:      {}", db.theme_dict().len());
                println!(
                    "Rating Range:       {} - {}",
                    db.header().min_rating,
                    db.header().max_rating
                );
                println!("Zstd Level:         {}", db.header().compression_level);
                println!(
                    "File Size:          {:.2} MB",
                    (std::fs::metadata(&cli.db)?.len()) as f64 / (1024.0 * 1024.0)
                );
            } else if ver == LPDB_VERSION_V3 {
                let db = BlockCompressedDb::open(&cli.db)?;
                let load_time = start.elapsed();
                println!(
                    "Database:           {:?} (Format V3 - Row Block-Compressed)",
                    cli.db
                );
                println!(
                    "Load / MMap Time:   {:.3} ms",
                    load_time.as_secs_f64() * 1000.0
                );
                println!("Total Puzzles:      {}", db.len());
                println!(
                    "Rating Range:       {} - {}",
                    db.header().min_rating,
                    db.header().max_rating
                );
                println!(
                    "File Size:          {:.2} MB",
                    (std::fs::metadata(&cli.db)?.len()) as f64 / (1024.0 * 1024.0)
                );
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                let load_time = start.elapsed();
                println!(
                    "Database:           {:?} (Format V2 - Direct Fixed-Record MMap)",
                    cli.db
                );
                println!(
                    "Load / MMap Time:   {:.3} ms",
                    load_time.as_secs_f64() * 1000.0
                );
                println!("Total Puzzles:      {}", db.len());
                println!(
                    "Rating Range:       {} - {}",
                    db.header().min_rating,
                    db.header().max_rating
                );
                println!(
                    "File Size:          {:.2} MB",
                    (std::fs::metadata(&cli.db)?.len()) as f64 / (1024.0 * 1024.0)
                );
            }
        }

        Commands::Get {
            id,
            index,
            board,
            json,
        } => {
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
                Some(p) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&p)?);
                    } else {
                        print_puzzle(&p, board);
                    }
                }
                None => {
                    if json {
                        println!("null");
                    } else {
                        println!("Puzzle not found.");
                    }
                }
            }
        }

        Commands::Query {
            min_rating,
            max_rating,
            themes,
            any_themes,
            exclude_themes,
            exclude_setup_move,
            limit,
            json,
            pgn,
            csv,
        } => {
            let ver = detect_db_version(&cli.db);
            let req_mask = themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let any_mask = any_themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let excl_mask = exclude_themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let criteria = QueryCriteria {
                min_rating,
                max_rating,
                required_themes: req_mask,
                any_themes: any_mask,
                excluded_themes: excl_mask,
            };

            let mut results = if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                db.query_puzzles(&criteria, limit)
            } else if ver == LPDB_VERSION_V3 {
                let db = BlockCompressedDb::open(&cli.db)?;
                db.query_puzzles(&criteria, limit)
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                db.query_puzzles(&criteria, limit)
            };

            if exclude_setup_move {
                results = results
                    .into_iter()
                    .map(|p| Exporter::puzzle_without_setup_move(&p))
                    .collect();
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else if pgn {
                for p in &results {
                    print!(
                        "{}",
                        Exporter::puzzle_to_pgn_with_options(p, !exclude_setup_move)
                    );
                }
            } else if csv {
                print!("PuzzleId,FEN,Moves,Rating,Themes\n");
                for p in &results {
                    print!("{}", Exporter::puzzle_to_csv_line(p));
                }
            } else {
                for p in &results {
                    println!(
                        "[{}] Rating: {:4} | Themes: {:<35} | Moves: {}",
                        p.id,
                        p.rating,
                        p.themes.join(", "),
                        p.moves.join(" ")
                    );
                }
            }
        }

        Commands::Random {
            min_rating,
            max_rating,
            themes,
            exclude_themes,
            exclude_setup_move,
            count,
            json,
            pgn,
        } => {
            let ver = detect_db_version(&cli.db);
            let req_mask = themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let excl_mask = exclude_themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let criteria = QueryCriteria {
                min_rating,
                max_rating,
                required_themes: req_mask,
                any_themes: None,
                excluded_themes: excl_mask,
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
            } else if ver == LPDB_VERSION_V3 {
                let db = BlockCompressedDb::open(&cli.db)?;
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

            if exclude_setup_move {
                results = results
                    .into_iter()
                    .map(|p| Exporter::puzzle_without_setup_move(&p))
                    .collect();
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else if pgn {
                for p in &results {
                    print!(
                        "{}",
                        Exporter::puzzle_to_pgn_with_options(p, !exclude_setup_move)
                    );
                }
            } else {
                for p in &results {
                    print_puzzle(p, true);
                }
            }
        }

        Commands::Export {
            output,
            format,
            min_rating,
            max_rating,
            themes,
            any_themes,
            exclude_themes,
            exclude_setup_move,
            sample,
            limit,
        } => {
            let ver = detect_db_version(&cli.db);
            let req_mask = themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let any_mask = any_themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let excl_mask = exclude_themes
                .as_deref()
                .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
            let criteria = QueryCriteria {
                min_rating,
                max_rating,
                required_themes: req_mask,
                any_themes: any_mask,
                excluded_themes: excl_mask,
            };

            let export_limit = limit.unwrap_or(usize::MAX);
            println!("Exporting puzzles from {:?}...", cli.db);
            let start = Instant::now();

            let puzzles = if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                if sample && limit.is_some() {
                    // Fetch all matching puzzles across the rating range, then sample uniformly
                    let all = db.query_puzzles(&criteria, usize::MAX);
                    if all.len() <= export_limit {
                        all
                    } else {
                        let step = all.len() as f64 / export_limit as f64;
                        (0..export_limit)
                            .map(|i| all[(i as f64 * step) as usize].clone())
                            .collect()
                    }
                } else {
                    db.query_puzzles(&criteria, export_limit)
                }
            } else if ver == LPDB_VERSION_V3 {
                let db = BlockCompressedDb::open(&cli.db)?;
                if sample && limit.is_some() {
                    let all = db.query_puzzles(&criteria, usize::MAX);
                    if all.len() <= export_limit {
                        all
                    } else {
                        let step = all.len() as f64 / export_limit as f64;
                        (0..export_limit)
                            .map(|i| all[(i as f64 * step) as usize].clone())
                            .collect()
                    }
                } else {
                    db.query_puzzles(&criteria, export_limit)
                }
            } else {
                let db = PuzzleDatabase::open(&cli.db)?;
                if sample && limit.is_some() {
                    let all = db.query_puzzles(&criteria, usize::MAX);
                    if all.len() <= export_limit {
                        all
                    } else {
                        let step = all.len() as f64 / export_limit as f64;
                        (0..export_limit)
                            .map(|i| all[(i as f64 * step) as usize].clone())
                            .collect()
                    }
                } else {
                    db.query_puzzles(&criteria, export_limit)
                }
            };

            let is_csv = match format.as_deref() {
                Some("csv") | Some("CSV") => true,
                Some("pgn") | Some("PGN") => false,
                _ => {
                    if let Some(ext) = output.extension().and_then(|s| s.to_str()) {
                        ext.eq_ignore_ascii_case("csv")
                    } else {
                        false
                    }
                }
            };

            let include_setup = !exclude_setup_move;
            let exported_count = if is_csv {
                Exporter::export_to_csv_file(&puzzles, &output, include_setup)?
            } else {
                Exporter::export_to_pgn_file(&puzzles, &output, include_setup)?
            };

            let elapsed = start.elapsed();
            println!(
                "Successfully exported {} puzzles to {:?} in {:.2}s ({:.2} MB)",
                exported_count,
                output,
                elapsed.as_secs_f64(),
                std::fs::metadata(&output)?.len() as f64 / (1024.0 * 1024.0)
            );
        }

        Commands::Bench { iterations } => {
            let ver = detect_db_version(&cli.db);
            println!("Running benchmarks on {:?} (Version: {})...", cli.db, ver);
            let t0 = Instant::now();

            if ver == LPDB_VERSION_V4 {
                let db = ColumnarDb::open(&cli.db)?;
                let open_duration = t0.elapsed();
                println!(
                    "V4 Header & Block Index MMap Open: {:.3} ms",
                    open_duration.as_secs_f64() * 1000.0
                );
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
                println!(
                    "V2 Zero-Copy MMap Open: {:.3} ms",
                    open_duration.as_secs_f64() * 1000.0
                );
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

        Commands::FetchGames {
            puzzles,
            game_ids,
            from_db,
            min_rating,
            max_rating,
            themes,
            random,
            limit,
            output,
            compression_level,
            token,
            yes,
            pause_every,
            pause_seconds,
            json_progress,
        } => {
            let mut puzzle_ids = Vec::new();
            let mut game_id_list = Vec::new();

            if let Some(p) = puzzles {
                puzzle_ids.extend(parse_id_list(&p));
            }

            if let Some(g) = game_ids {
                game_id_list.extend(parse_id_list(&g));
            }

            if let Some(db_path) = from_db {
                println!("Reading puzzle IDs from database: {:?}", db_path);
                let ver = detect_db_version(&db_path);
                let req_mask = themes
                    .as_deref()
                    .map(|t| ThemeMask::from_names(t.split(',').map(|s| s.trim())));
                let criteria = QueryCriteria {
                    min_rating,
                    max_rating,
                    required_themes: req_mask,
                    any_themes: None,
                    excluded_themes: None,
                };

                let query_limit = limit.unwrap_or(usize::MAX);
                let mut found_puzzles = if ver == LPDB_VERSION_V4 {
                    let db = ColumnarDb::open(&db_path)?;
                    db.query_puzzles(&criteria, query_limit)
                } else if ver == LPDB_VERSION_V3 {
                    let db = BlockCompressedDb::open(&db_path)?;
                    db.query_puzzles(&criteria, query_limit)
                } else {
                    let db = PuzzleDatabase::open(&db_path)?;
                    db.query_puzzles(&criteria, query_limit)
                };

                if random && !found_puzzles.is_empty() {
                    use rand::seq::SliceRandom;
                    let mut rng = rand::thread_rng();
                    found_puzzles.shuffle(&mut rng);
                }

                if let Some(lim) = limit {
                    found_puzzles.truncate(lim);
                }

                for p in found_puzzles {
                    puzzle_ids.push(p.id);
                }
            }

            if let Some(lim) = limit {
                puzzle_ids.truncate(lim);
            }

            let total_count = puzzle_ids.len() + game_id_list.len();
            if total_count == 0 {
                eprintln!("Error: No puzzle IDs or game IDs specified. Use --puzzles, --game-ids, or --from-db");
                std::process::exit(1);
            }

            // User confirmation if > 1000 games
            if total_count > 1000 && !yes {
                println!(
                    "Warning: You have requested to download {} games from Lichess.",
                    total_count
                );
                println!("This may take considerable time and consume API rate limits.");
                print!("Do you want to proceed? [y/N]: ");
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let ans = input.trim();
                if !ans.eq_ignore_ascii_case("y") && !ans.eq_ignore_ascii_case("yes") {
                    println!("Download cancelled by user.");
                    return Ok(());
                }
            }

            let opts = GameDownloadOptions {
                puzzle_ids,
                game_ids: game_id_list,
                limit,
                output_path: output.display().to_string(),
                compression_level,
                token,
                pause_every,
                pause_duration_secs: pause_seconds,
                json_progress,
            };

            let stats = download_and_build_lgdb(opts)?;
            println!("--------------------------------------------------");
            println!("Download Summary:");
            println!("  Total Attempted:    {}", stats.attempted);
            println!("  Successfully Saved: {}", stats.succeeded);
            println!("  Failed / Missing:   {}", stats.failed);
            println!("  Total Plies:        {}", stats.db_stats.total_plies);
            println!("  Elapsed Time:       {:.2}s", stats.duration.as_secs_f64());
            println!(
                "  Compressed DB Size: {:.2} KB ({:.1} bytes/game)",
                stats.db_stats.total_file_size as f64 / 1024.0,
                stats.db_stats.total_file_size as f64 / stats.succeeded.max(1) as f64
            );
            let comp_factor = if stats.db_stats.compression_ratio > 0.0 {
                1.0 / stats.db_stats.compression_ratio
            } else {
                1.0
            };
            println!("  Compression Ratio:  {:.2}x", comp_factor);
            println!("--------------------------------------------------");
        }

        Commands::InspectGames {
            games,
            limit,
            index,
            puzzle_id,
            game_id,
            json,
            export_pgn,
        } => {
            let lgdb = Lgdb::open(&games)?;
            let file_size = std::fs::metadata(&games)?.len();
            let header = lgdb.header();

            if json {
                if let Some(i) = index {
                    if let Some(g) = lgdb.get(i) {
                        let moves_uci: Vec<String> = g.moves.iter().map(|m| m.to_uci()).collect();
                        let obj = serde_json::json!({
                            "index": i,
                            "puzzle_id": g.puzzle_id,
                            "game_id": g.game_id,
                            "date": g.date,
                            "result": g.result.as_str(),
                            "plies": g.moves.len(),
                            "moves": moves_uci,
                        });
                        println!("{}", serde_json::to_string_pretty(&obj)?);
                    } else {
                        println!("null");
                    }
                    return Ok(());
                }

                if let Some(ref pid) = puzzle_id {
                    if let Some(g) = lgdb.find_by_puzzle_id(pid) {
                        let moves_uci: Vec<String> = g.moves.iter().map(|m| m.to_uci()).collect();
                        let obj = serde_json::json!({
                            "puzzle_id": g.puzzle_id,
                            "game_id": g.game_id,
                            "date": g.date,
                            "result": g.result.as_str(),
                            "plies": g.moves.len(),
                            "moves": moves_uci,
                        });
                        println!("{}", serde_json::to_string_pretty(&obj)?);
                    } else {
                        println!("null");
                    }
                    return Ok(());
                }

                if let Some(ref gid) = game_id {
                    if let Some(g) = lgdb.find_by_game_id(gid) {
                        let moves_uci: Vec<String> = g.moves.iter().map(|m| m.to_uci()).collect();
                        let obj = serde_json::json!({
                            "puzzle_id": g.puzzle_id,
                            "game_id": g.game_id,
                            "date": g.date,
                            "result": g.result.as_str(),
                            "plies": g.moves.len(),
                            "moves": moves_uci,
                        });
                        println!("{}", serde_json::to_string_pretty(&obj)?);
                    } else {
                        println!("null");
                    }
                    return Ok(());
                }

                let mut list = Vec::new();
                for i in 0..limit.min(lgdb.len()) {
                    if let Some(g) = lgdb.get(i) {
                        let moves_uci: Vec<String> = g.moves.iter().map(|m| m.to_uci()).collect();
                        list.push(serde_json::json!({
                            "index": i,
                            "puzzle_id": g.puzzle_id,
                            "game_id": g.game_id,
                            "date": g.date,
                            "result": g.result.as_str(),
                            "plies": g.moves.len(),
                            "moves": moves_uci,
                        }));
                    }
                }
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }

            println!("Opening game database: {:?}", games);
            println!("==================================================");
            println!(
                "Database:           {:?} (LGDB Format v{})",
                games, header.version
            );
            println!("Total Games:        {}", lgdb.len());
            println!("Total Moves (Plies):{}", header.uncompressed_size / 2);
            println!(
                "File Size:          {:.2} KB ({:.1} bytes/game)",
                file_size as f64 / 1024.0,
                file_size as f64 / lgdb.len().max(1) as f64
            );
            println!(
                "Index Table Size:   {:.2} KB",
                (header.game_count as u64 * 24) as f64 / 1024.0
            );
            println!(
                "Compressed Moves:   {:.2} KB",
                header.compressed_size as f64 / 1024.0
            );
            println!(
                "Uncompressed Moves: {:.2} KB",
                header.uncompressed_size as f64 / 1024.0
            );
            let ratio = if header.compressed_size > 0 {
                header.uncompressed_size as f64 / header.compressed_size as f64
            } else {
                1.0
            };
            println!("Move Compression:   {:.2}x Zstandard", ratio);
            println!("==================================================");

            if let Some(pgn_path) = export_pgn {
                println!("Exporting {} games to PGN: {:?}...", lgdb.len(), pgn_path);
                let file = std::fs::File::create(&pgn_path)?;
                let mut writer = std::io::BufWriter::new(file);
                for i in 0..lgdb.len() {
                    if let Some(pgn_game) = lgdb.to_pgn(i) {
                        writer.write_all(pgn_game.as_bytes())?;
                        writer.write_all(b"\n")?;
                    }
                }
                writer.flush()?;
                println!("Successfully exported to {:?}", pgn_path);
            }

            if limit > 0 && !lgdb.is_empty() {
                println!("\nPreviewing first {} games:", limit.min(lgdb.len()));
                for i in 0..limit.min(lgdb.len()) {
                    let g = lgdb.get(i).unwrap();
                    let moves_preview: Vec<String> =
                        g.moves.iter().take(6).map(|m| m.to_uci()).collect();
                    let ellipsis = if g.moves.len() > 6 { "..." } else { "" };
                    println!(
                        "  [{:>3}] Puzzle: {:<6} | Game: {:<8} | Date: {:<10} | Result: {:<7} | Plies: {:3} | Moves: {}{}",
                        i + 1,
                        g.puzzle_id,
                        g.game_id,
                        g.date,
                        g.result.as_str(),
                        g.moves.len(),
                        moves_preview.join(" "),
                        ellipsis
                    );
                }
            }
        }
    }

    Ok(())
}
