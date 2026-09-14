//! Fast parallel builder for V4 columnar block-compressed databases.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::board::CompactBoard;
use crate::columnar_db::{BlockEntryV4, DbHeaderV4, DEFAULT_V4_BLOCK_SIZE, LPDB_MAGIC, LPDB_VERSION_V4};
use crate::fen::PackedFen;
use crate::id::{encode_base62_id, is_standard_base62_5char};
use crate::moves::{encode_moves, PackedMove};
use crate::theme::ThemeMask;

pub struct ColumnarBuilderOptions {
    pub min_rating: Option<u16>,
    pub max_rating: Option<u16>,
    pub min_popularity: Option<i8>,
    pub min_plays: Option<u32>,
    pub block_size: usize,
    pub compression_level: i32,
    pub limit: Option<usize>,
}

impl Default for ColumnarBuilderOptions {
    fn default() -> Self {
        Self {
            min_rating: None,
            max_rating: None,
            min_popularity: None,
            min_plays: None,
            block_size: DEFAULT_V4_BLOCK_SIZE,
            compression_level: 19,
            limit: None,
        }
    }
}

pub struct ColumnarBuilderStats {
    pub total_puzzles: usize,
    pub total_blocks: usize,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
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

#[derive(Clone)]
struct ParsedPuzzleItem {
    id_str: String,
    id_base62: u32,
    is_standard_id: bool,
    rating: u16,
    theme_mask: ThemeMask,
    fen_state: u16,
    fen_halfmove: u8,
    board_occupied: u64,
    board_pieces: Vec<u8>,
    moves: Vec<PackedMove>,
}

impl RawPuzzleRow {
    fn process(self) -> Option<ParsedPuzzleItem> {
        let packed_fen = PackedFen::encode(&self.fen).ok()?;
        let compact_board = CompactBoard::from_board_nibbles(&packed_fen.board);
        let moves = encode_moves(&self.moves);
        let theme_mask = if self.themes.chars().any(|c| c.is_ascii_digit()) {
            ThemeMask::from_sqlite_ids_str(&self.themes)
        } else {
            ThemeMask::from_csv_themes_str(&self.themes)
        };

        let is_standard_id = is_standard_base62_5char(&self.id);
        let id_base62 = if is_standard_id {
            encode_base62_id(&self.id)
        } else {
            0 // Will be assigned to overflow string pool
        };

        let piece_count = compact_board.occupied.count_ones() as usize;
        let packed_piece_len = (piece_count + 1) / 2;
        let board_pieces = compact_board.pieces[..packed_piece_len].to_vec();

        Some(ParsedPuzzleItem {
            id_str: self.id,
            id_base62,
            is_standard_id,
            rating: self.rating,
            theme_mask,
            fen_state: packed_fen.state,
            fen_halfmove: packed_fen.halfmove,
            board_occupied: compact_board.occupied,
            board_pieces,
            moves,
        })
    }
}

pub struct ColumnarDbBuilder;

impl ColumnarDbBuilder {
    /// Builds a `.lpdb` V4 database from SQLite with adaptive fallback.
    pub fn build_from_sqlite(
        sqlite_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        options: ColumnarBuilderOptions,
    ) -> io::Result<ColumnarBuilderStats> {
        let start_time = Instant::now();
        let conn = rusqlite::Connection::open(sqlite_path.as_ref())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let mut conditions = Vec::new();
        if let Some(min_r) = options.min_rating {
            conditions.push(format!("Rating >= {}", min_r));
        }
        if let Some(max_r) = options.max_rating {
            conditions.push(format!("Rating <= {}", max_r));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = match options.limit {
            Some(lim) => format!("LIMIT {}", lim),
            None => String::new(),
        };

        let count_query = format!(
            "SELECT COUNT(*) FROM (SELECT 1 FROM puzzles {} {})",
            where_clause, limit_clause
        );
        let total_count: usize = conn
            .query_row(&count_query, [], |row| row.get(0))
            .unwrap_or(0);

        println!(
            "Processing {} puzzles from SQLite (Filters: Rating {:?}..{:?})...",
            total_count, options.min_rating, options.max_rating
        );

        let query = format!(
            "SELECT PuzzleId, FEN, Moves, Rating, Themes FROM puzzles {} {}",
            where_clause, limit_clause
        );

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
        let mut parsed_items: Vec<ParsedPuzzleItem> = Vec::with_capacity(total_count);

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
                    let chunk_results: Vec<ParsedPuzzleItem> = rows
                        .par_drain(..)
                        .filter_map(|r| r.process())
                        .collect();
                    pb.inc(chunk_results.len() as u64);
                    parsed_items.extend(chunk_results);
                }
            }
        }

        if !rows.is_empty() {
            let chunk_results: Vec<ParsedPuzzleItem> = rows
                .par_drain(..)
                .filter_map(|r| r.process())
                .collect();
            pb.inc(chunk_results.len() as u64);
            parsed_items.extend(chunk_results);
        }

        pb.finish_with_message("Parsing completed");

        Self::finalize_database(parsed_items, output_path, options, start_time)
    }

    /// Builds a `.lpdb` V4 database directly from an official Lichess Zstandard compressed CSV.
    /// Cleans unnecessary columns (RatingDeviation, GameUrl, OpeningTags) and filters on the fly.
    pub fn build_from_csv_zst(
        zst_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        options: ColumnarBuilderOptions,
    ) -> io::Result<ColumnarBuilderStats> {
        let start_time = Instant::now();
        let file = File::open(zst_path.as_ref())?;
        let decoder = zstd::Decoder::new(file)?;
        let reader = BufReader::new(decoder);
        let mut csv_reader = csv::ReaderBuilder::new()
            .has_headers(true)
            .from_reader(reader);

        println!("Streaming and processing CSV rows from {:?}...", zst_path.as_ref());
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} [{elapsed_precise}] {pos} puzzles processed ({msg})")
                .unwrap(),
        );

        let mut rows: Vec<RawPuzzleRow> = Vec::with_capacity(50_000);
        let mut parsed_items: Vec<ParsedPuzzleItem> = Vec::new();
        let mut count = 0;

        for result in csv_reader.records() {
            let record = result.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            // Schema: 0:PuzzleId, 1:FEN, 2:Moves, 3:Rating, 4:RatingDeviation, 5:Popularity, 6:NbPlays, 7:Themes, 8:GameUrl, 9:OpeningTags
            if record.len() >= 8 {
                let rating = record[3].parse::<u16>().unwrap_or(1500);

                // Rating filtering
                if let Some(min_r) = options.min_rating {
                    if rating < min_r { continue; }
                }
                if let Some(max_r) = options.max_rating {
                    if rating > max_r { continue; }
                }

                // Popularity filtering
                if let Some(min_pop) = options.min_popularity {
                    let pop = record[5].parse::<i8>().unwrap_or(0);
                    if pop < min_pop { continue; }
                }

                // Play count filtering
                if let Some(min_p) = options.min_plays {
                    let plays = record[6].parse::<u32>().unwrap_or(0);
                    if plays < min_p { continue; }
                }

                let id = record[0].to_string();
                let fen = record[1].to_string();
                let moves = record[2].to_string();
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
                    let chunk_results: Vec<ParsedPuzzleItem> = rows
                        .par_drain(..)
                        .filter_map(|r| r.process())
                        .collect();
                    pb.inc(chunk_results.len() as u64);
                    parsed_items.extend(chunk_results);
                }

                if let Some(lim) = options.limit {
                    if count >= lim {
                        break;
                    }
                }
            }
        }

        if !rows.is_empty() {
            let chunk_results: Vec<ParsedPuzzleItem> = rows
                .par_drain(..)
                .filter_map(|r| r.process())
                .collect();
            pb.inc(chunk_results.len() as u64);
            parsed_items.extend(chunk_results);
        }

        pb.finish_with_message("CSV stream completed");

        Self::finalize_database(parsed_items, output_path, options, start_time)
    }

    fn finalize_database(
        mut items: Vec<ParsedPuzzleItem>,
        output_path: impl AsRef<Path>,
        options: ColumnarBuilderOptions,
        start_time: Instant,
    ) -> io::Result<ColumnarBuilderStats> {
        println!("Sorting {} records by rating...", items.len());
        items.par_sort_unstable_by_key(|item| item.rating);

        let total_puzzles = items.len();
        let min_rating = items.first().map(|r| r.rating).unwrap_or(0);
        let max_rating = items.last().map(|r| r.rating).unwrap_or(0);

        println!("Building theme dictionary...");
        let mut theme_dict: Vec<ThemeMask> = Vec::new();
        let mut theme_map: HashMap<u128, u32> = HashMap::new();

        for item in &items {
            theme_map.entry(item.theme_mask.0).or_insert_with(|| {
                let id = theme_dict.len() as u32;
                theme_dict.push(item.theme_mask);
                id
            });
        }

        let unique_themes = theme_dict.len();
        println!("Deduplicated Theme Dictionary: {} unique theme combinations", unique_themes);

        let chunk_size = options.block_size;
        let num_blocks = (total_puzzles + chunk_size - 1) / chunk_size;
        let comp_level = options.compression_level;

        println!(
            "Compressing {} blocks in parallel (Block size: {}, Zstd level: {})...",
            num_blocks, chunk_size, comp_level
        );

        let chunks: Vec<Vec<ParsedPuzzleItem>> = items
            .chunks(chunk_size)
            .map(|c| c.to_vec())
            .collect();

        struct CompressedColumnarBlock {
            entry: BlockEntryV4,
            compressed_data: Vec<u8>,
        }

        let compressed_results: Vec<CompressedColumnarBlock> = chunks
            .into_par_iter()
            .map(|chunk| {
                let puzzle_count = chunk.len() as u16;
                let min_r = chunk.first().map(|p| p.rating).unwrap_or(0);
                let max_r = chunk.last().map(|p| p.rating).unwrap_or(0);

                let mut payload = Vec::with_capacity(chunk.len() * 45);
                let mut overflow_ids = Vec::new();

                // 1. Header: count (u32)
                payload.extend_from_slice(&(chunk.len() as u32).to_le_bytes());

                // 2. Column: IDs with adaptive escape
                for p in &chunk {
                    if p.is_standard_id {
                        payload.extend_from_slice(&p.id_base62.to_le_bytes());
                    } else {
                        let overflow_idx = overflow_ids.len() as u32;
                        overflow_ids.push(p.id_str.clone());
                        let escaped = 0x8000_0000 | (overflow_idx & 0x7FFF_FFFF);
                        payload.extend_from_slice(&escaped.to_le_bytes());
                    }
                }

                // 3. Column: Ratings (Delta-encoded)
                let mut prev_r = chunk.first().map(|p| p.rating).unwrap_or(0);
                payload.extend_from_slice(&prev_r.to_le_bytes());
                for p in chunk.iter().skip(1) {
                    let delta = p.rating.wrapping_sub(prev_r);
                    payload.extend_from_slice(&delta.to_le_bytes());
                    prev_r = p.rating;
                }

                // 4. Column: Theme IDs
                for p in &chunk {
                    let t_id = theme_map[&p.theme_mask.0];
                    payload.extend_from_slice(&t_id.to_le_bytes());
                }

                // 5. Column: FEN State & Halfmove
                for p in &chunk {
                    payload.extend_from_slice(&p.fen_state.to_le_bytes());
                    payload.push(p.fen_halfmove);
                }

                // 6. Column: Board Occupancy
                for p in &chunk {
                    payload.extend_from_slice(&p.board_occupied.to_le_bytes());
                }

                // 7. Column: Piece Stream
                for p in &chunk {
                    payload.push(p.board_pieces.len() as u8);
                }
                for p in &chunk {
                    payload.extend_from_slice(&p.board_pieces);
                }

                // 8. Column: Move Stream
                for p in &chunk {
                    payload.push(p.moves.len() as u8);
                }
                for p in &chunk {
                    payload.extend_from_slice(bytemuck::cast_slice(&p.moves));
                }

                // 9. Adaptive Overflow String Pool
                if !overflow_ids.is_empty() {
                    payload.extend_from_slice(&(overflow_ids.len() as u16).to_le_bytes());
                    for s in &overflow_ids {
                        let bytes = s.as_bytes();
                        payload.push(bytes.len().min(255) as u8);
                        payload.extend_from_slice(&bytes[..bytes.len().min(255)]);
                    }
                }

                let uncompressed_size = payload.len() as u32;
                let compressed_data = zstd::encode_all(&payload[..], comp_level).unwrap();
                let compressed_size = compressed_data.len() as u32;

                CompressedColumnarBlock {
                    entry: BlockEntryV4 {
                        file_offset: 0,
                        compressed_size,
                        uncompressed_size,
                        puzzle_count,
                        min_rating: min_r,
                        max_rating: max_r,
                        flags: if overflow_ids.is_empty() { 0 } else { 1 },
                    },
                    compressed_data,
                }
            })
            .collect();

        println!("Writing V4 database to {:?}...", output_path.as_ref());
        let out_file = File::create(output_path.as_ref())?;
        let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, out_file);

        let theme_dict_offset = std::mem::size_of::<DbHeaderV4>() as u64;
        let theme_dict_size = (theme_dict.len() * std::mem::size_of::<ThemeMask>()) as u64;

        let block_index_offset = theme_dict_offset + theme_dict_size;
        let block_index_size = (num_blocks * std::mem::size_of::<BlockEntryV4>()) as u64;

        let blocks_payload_offset = block_index_offset + block_index_size;

        let header = DbHeaderV4 {
            magic: *LPDB_MAGIC,
            version: LPDB_VERSION_V4,
            puzzle_count: total_puzzles as u64,
            block_count: num_blocks as u32,
            block_size: chunk_size as u32,
            block_index_offset,
            theme_dict_offset,
            theme_dict_count: unique_themes as u32,
            min_rating,
            max_rating,
            compression_level: comp_level as u8,
            reserved: [0u8; 15],
        };

        // 1. Write Header
        writer.write_all(bytemuck::bytes_of(&header))?;

        // 2. Write Theme Dictionary
        let theme_dict_bytes: &[u8] = bytemuck::cast_slice(&theme_dict);
        writer.write_all(theme_dict_bytes)?;

        // Prepare block entries with exact file offsets
        let mut current_offset = blocks_payload_offset;
        let mut block_entries = Vec::with_capacity(num_blocks);
        let mut total_uncompressed_bytes = 0u64;

        for res in &compressed_results {
            let mut entry = res.entry;
            entry.file_offset = current_offset;
            current_offset += res.compressed_data.len() as u64;
            total_uncompressed_bytes += entry.uncompressed_size as u64;
            block_entries.push(entry);
        }

        // 3. Write Block Index Table
        let index_bytes: &[u8] = bytemuck::cast_slice(&block_entries);
        writer.write_all(index_bytes)?;

        // 4. Write Compressed Blocks Payload
        for res in compressed_results {
            writer.write_all(&res.compressed_data)?;
        }

        writer.flush()?;
        let total_bytes_written = current_offset;
        let duration_secs = start_time.elapsed().as_secs_f64();

        println!(
            "Successfully created LPDB V4 binary! Size: {:.2} MB (Uncompressed: {:.2} MB, Saved: {:.1}%) in {:.2}s",
            total_bytes_written as f64 / (1024.0 * 1024.0),
            total_uncompressed_bytes as f64 / (1024.0 * 1024.0),
            (1.0 - (total_bytes_written as f64 / total_uncompressed_bytes as f64)) * 100.0,
            duration_secs
        );

        Ok(ColumnarBuilderStats {
            total_puzzles,
            total_blocks: num_blocks,
            uncompressed_size: total_uncompressed_bytes,
            compressed_size: total_bytes_written,
            min_rating,
            max_rating,
            duration_secs,
        })
    }
}
