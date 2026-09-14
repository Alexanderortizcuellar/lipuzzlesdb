//! Fast binary database builder (V2 format) from SQLite or CSV/ZST sources.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::board::CompactBoard;
use crate::db::{DbHeader, LPDB_MAGIC, LPDB_VERSION};
use crate::fen::PackedFen;
use crate::moves::{encode_moves, PackedMove};
use crate::record::PuzzleRecord;
use crate::theme::ThemeMask;

pub struct BuilderStats {
    pub total_puzzles: usize,
    pub total_moves: usize,
    pub unique_themes: usize,
    pub total_bytes_written: u64,
    pub min_rating: u16,
    pub max_rating: u16,
    pub duration_secs: f64,
}

struct RawPuzzleRow {
    id: String,
    fen: String,
    moves: String,
    rating: u16,
    themes: String,
}

struct ParsedItem {
    id: String,
    compact_board: CompactBoard,
    rating: u16,
    theme_mask: ThemeMask,
    fen_state: u16,
    fen_halfmove: u8,
    moves: Vec<PackedMove>,
}

impl RawPuzzleRow {
    fn process(self) -> Option<ParsedItem> {
        let packed_fen = PackedFen::encode(&self.fen).ok()?;
        let compact_board = CompactBoard::from_board_nibbles(&packed_fen.board);
        let moves = encode_moves(&self.moves);
        let theme_mask = if self.themes.chars().any(|c| c.is_ascii_digit()) {
            ThemeMask::from_sqlite_ids_str(&self.themes)
        } else {
            ThemeMask::from_csv_themes_str(&self.themes)
        };

        Some(ParsedItem {
            id: self.id,
            compact_board,
            rating: self.rating,
            theme_mask,
            fen_state: packed_fen.state,
            fen_halfmove: packed_fen.halfmove,
            moves,
        })
    }
}

pub struct DbBuilder;

impl DbBuilder {
    /// Builds a `.lpdb` database from an SQLite file.
    pub fn build_from_sqlite(
        sqlite_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        limit: Option<usize>,
    ) -> io::Result<BuilderStats> {
        let start_time = Instant::now();
        let conn = rusqlite::Connection::open(sqlite_path.as_ref())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        println!("Querying SQLite database...");
        let count_query = match limit {
            Some(lim) => format!("SELECT COUNT(*) FROM (SELECT 1 FROM puzzles LIMIT {})", lim),
            None => "SELECT COUNT(*) FROM puzzles".to_string(),
        };
        let total_count: usize = conn
            .query_row(&count_query, [], |row| row.get(0))
            .unwrap_or(0);

        println!("Processing {} puzzles from SQLite...", total_count);

        let query = match limit {
            Some(lim) => format!("SELECT PuzzleId, FEN, Moves, Rating, Themes FROM puzzles LIMIT {}", lim),
            None => "SELECT PuzzleId, FEN, Moves, Rating, Themes FROM puzzles".to_string(),
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let pb = ProgressBar::new(total_count as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut rows: Vec<RawPuzzleRow> = Vec::with_capacity(total_count.min(100_000));
        let mut parsed_items: Vec<ParsedItem> = Vec::with_capacity(total_count);

        let puzzle_iter = stmt
            .query_map([], |row| {
                Ok(RawPuzzleRow {
                    id: row.get(0)?,
                    fen: row.get(1)?,
                    moves: row.get(2)?,
                    rating: row.get::<_, i64>(3)? as u16,
                    themes: row.get(4)?,
                })
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        for row_res in puzzle_iter {
            if let Ok(row) = row_res {
                rows.push(row);
                if rows.len() >= 50_000 {
                    let chunk_results: Vec<ParsedItem> = rows
                        .par_drain(..)
                        .filter_map(|r| r.process())
                        .collect();
                    pb.inc(chunk_results.len() as u64);
                    parsed_items.extend(chunk_results);
                }
            }
        }

        if !rows.is_empty() {
            let chunk_results: Vec<ParsedItem> = rows
                .par_drain(..)
                .filter_map(|r| r.process())
                .collect();
            pb.inc(chunk_results.len() as u64);
            parsed_items.extend(chunk_results);
        }

        pb.finish_with_message("Parsing completed");

        Self::finalize_database(parsed_items, output_path, start_time)
    }

    /// Builds a `.lpdb` database from a Zstandard compressed CSV file.
    pub fn build_from_csv_zst(
        zst_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        limit: Option<usize>,
    ) -> io::Result<BuilderStats> {
        let start_time = Instant::now();
        let file = File::open(zst_path)?;
        let decoder = zstd::Decoder::new(file)?;
        let reader = BufReader::new(decoder);
        let mut csv_reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(reader);

        println!("Streaming and processing CSV rows...");
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {pos} puzzles processed ({msg})")
                .unwrap(),
        );

        let mut rows: Vec<RawPuzzleRow> = Vec::with_capacity(50_000);
        let mut parsed_items: Vec<ParsedItem> = Vec::new();
        let mut count = 0;

        for result in csv_reader.records() {
            let record = result.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            if record.len() >= 8 {
                let id = record[0].to_string();
                let fen = record[1].to_string();
                let moves = record[2].to_string();
                let rating = record[3].parse::<u16>().unwrap_or(1500);
                let themes = record[7].to_string();

                rows.push(RawPuzzleRow {
                    id,
                    fen,
                    moves,
                    rating,
                    themes,
                });
                count += 1;

                if rows.len() >= 50_000 {
                    let chunk_results: Vec<ParsedItem> = rows
                        .par_drain(..)
                        .filter_map(|r| r.process())
                        .collect();
                    pb.inc(chunk_results.len() as u64);
                    parsed_items.extend(chunk_results);
                }

                if let Some(lim) = limit {
                    if count >= lim {
                        break;
                    }
                }
            }
        }

        if !rows.is_empty() {
            let chunk_results: Vec<ParsedItem> = rows
                .par_drain(..)
                .filter_map(|r| r.process())
                .collect();
            pb.inc(chunk_results.len() as u64);
            parsed_items.extend(chunk_results);
        }

        pb.finish_with_message("CSV stream completed");

        Self::finalize_database(parsed_items, output_path, start_time)
    }

    fn finalize_database(
        mut items: Vec<ParsedItem>,
        output_path: impl AsRef<Path>,
        start_time: Instant,
    ) -> io::Result<BuilderStats> {
        println!("Sorting {} records by rating...", items.len());
        items.par_sort_unstable_by_key(|item| item.rating);

        let total_puzzles = items.len();
        let min_rating = items.first().map(|r| r.rating).unwrap_or(0);
        let max_rating = items.last().map(|r| r.rating).unwrap_or(0);

        println!("Building theme dictionary...");
        let mut theme_dict: Vec<ThemeMask> = Vec::new();
        let mut theme_map: HashMap<u128, u32> = HashMap::new();

        println!("Collation & encoding 40-byte records...");
        let mut records: Vec<PuzzleRecord> = Vec::with_capacity(total_puzzles);
        let mut move_pool: Vec<PackedMove> = Vec::new();

        for item in items {
            let theme_id = *theme_map.entry(item.theme_mask.0).or_insert_with(|| {
                let id = theme_dict.len() as u32;
                theme_dict.push(item.theme_mask);
                id
            });

            let move_offset = move_pool.len() as u32;
            let move_count = item.moves.len() as u8;
            move_pool.extend_from_slice(&item.moves);

            let record = PuzzleRecord::new(
                &item.id,
                &item.compact_board,
                item.rating,
                theme_id,
                move_offset,
                move_count,
                item.fen_state,
                item.fen_halfmove,
            );
            records.push(record);
        }

        let unique_themes = theme_dict.len();
        let total_moves = move_pool.len();
        println!(
            "Deduplicated to {} unique theme masks (Dict: {:.2} KB)",
            unique_themes,
            (unique_themes * std::mem::size_of::<ThemeMask>()) as f64 / 1024.0
        );
        println!("Total moves in pool: {}", total_moves);

        println!("Writing binary file to {:?}...", output_path.as_ref());
        let out_file = File::create(output_path.as_ref())?;
        let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, out_file);

        let theme_dict_offset = std::mem::size_of::<DbHeader>() as u64;
        let theme_dict_size = (theme_dict.len() * std::mem::size_of::<ThemeMask>()) as u64;

        let records_offset = theme_dict_offset + theme_dict_size;
        let records_size = (records.len() * std::mem::size_of::<PuzzleRecord>()) as u64;

        let move_pool_offset = records_offset + records_size;
        let move_pool_size = (move_pool.len() * std::mem::size_of::<PackedMove>()) as u64;

        let header = DbHeader {
            magic: *LPDB_MAGIC,
            version: LPDB_VERSION,
            puzzle_count: total_puzzles as u64,
            records_offset,
            move_pool_count: total_moves as u64,
            move_pool_offset,
            theme_dict_offset,
            theme_dict_count: unique_themes as u32,
            min_rating,
            max_rating,
            reserved: [0u8; 8],
        };

        // 1. Write Header
        writer.write_all(bytemuck::bytes_of(&header))?;

        // 2. Write Theme Dictionary
        let theme_dict_bytes: &[u8] = bytemuck::cast_slice(&theme_dict);
        writer.write_all(theme_dict_bytes)?;

        // 3. Write Records
        let records_bytes: &[u8] = bytemuck::cast_slice(&records);
        writer.write_all(records_bytes)?;

        // 4. Write Move Pool
        let move_pool_bytes: &[u8] = bytemuck::cast_slice(&move_pool);
        writer.write_all(move_pool_bytes)?;

        writer.flush()?;
        let total_bytes_written = std::mem::size_of::<DbHeader>() as u64
            + theme_dict_size
            + records_size
            + move_pool_size;
        let duration_secs = start_time.elapsed().as_secs_f64();

        println!(
            "Successfully created LPDB V2 binary! Size: {:.2} MB in {:.2}s",
            total_bytes_written as f64 / (1024.0 * 1024.0),
            duration_secs
        );

        Ok(BuilderStats {
            total_puzzles,
            total_moves,
            unique_themes,
            total_bytes_written,
            min_rating,
            max_rating,
            duration_secs,
        })
    }
}
