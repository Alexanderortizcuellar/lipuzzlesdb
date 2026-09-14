//! Fast parallel builder for V3 block-compressed databases.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::block_db::{BlockEntry, BlockPuzzleRecord, DbHeaderV3, DEFAULT_BLOCK_SIZE, LPDB_MAGIC, LPDB_VERSION_V3};
use crate::board::CompactBoard;
use crate::fen::PackedFen;
use crate::id::encode_base62_id;
use crate::moves::{encode_moves, PackedMove};
use crate::theme::ThemeMask;

pub struct BlockBuilderStats {
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

pub struct BlockDbBuilder;

impl BlockDbBuilder {
    pub fn build_from_sqlite(
        sqlite_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        limit: Option<usize>,
    ) -> io::Result<BlockBuilderStats> {
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

    fn finalize_database(
        mut items: Vec<ParsedItem>,
        output_path: impl AsRef<Path>,
        start_time: Instant,
    ) -> io::Result<BlockBuilderStats> {
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
        println!("Theme dictionary: {} unique themes", unique_themes);

        println!("Compressing blocks in parallel (512 puzzles/block)...");
        let chunk_size = DEFAULT_BLOCK_SIZE;
        let num_blocks = (total_puzzles + chunk_size - 1) / chunk_size;

        // Group items into chunks
        let chunks: Vec<Vec<ParsedItem>> = items
            .chunks(chunk_size)
            .map(|c| c.to_vec())
            .collect();

        struct CompressedBlockResult {
            entry: BlockEntry,
            compressed_data: Vec<u8>,
        }

        let compressed_results: Vec<CompressedBlockResult> = chunks
            .into_par_iter()
            .map(|chunk| {
                let puzzle_count = chunk.len() as u16;
                let min_r = chunk.first().map(|p| p.rating).unwrap_or(0);
                let max_r = chunk.last().map(|p| p.rating).unwrap_or(0);

                let mut records = Vec::with_capacity(chunk.len());
                let mut moves = Vec::new();

                for item in chunk {
                    let theme_id = theme_map[&item.theme_mask.0];
                    let move_local_offset = moves.len() as u16;
                    let move_count = item.moves.len() as u8;
                    moves.extend_from_slice(&item.moves);

                    let rec = BlockPuzzleRecord {
                        board_occupied: item.compact_board.occupied,
                        board_pieces: item.compact_board.pieces,
                        id_base62: encode_base62_id(&item.id),
                        theme_id,
                        move_local_offset,
                        move_count,
                        fen_halfmove: item.fen_halfmove,
                        fen_state: item.fen_state,
                        rating: item.rating,
                    };
                    records.push(rec);
                }

                // Serialize uncompressed payload: [puzzle_count: u32][move_count: u32][records][moves]
                let mut payload = Vec::new();
                payload.extend_from_slice(&(records.len() as u32).to_le_bytes());
                payload.extend_from_slice(&(moves.len() as u32).to_le_bytes());
                payload.extend_from_slice(bytemuck::cast_slice(&records));
                payload.extend_from_slice(bytemuck::cast_slice(&moves));

                let uncompressed_size = payload.len() as u32;

                // Compress with Zstd Level 3
                let compressed_data = zstd::encode_all(&payload[..], 3).unwrap();
                let compressed_size = compressed_data.len() as u32;

                CompressedBlockResult {
                    entry: BlockEntry {
                        file_offset: 0, // Assigned sequentially later
                        compressed_size,
                        uncompressed_size,
                        puzzle_count,
                        min_rating: min_r,
                        max_rating: max_r,
                        reserved: 0,
                    },
                    compressed_data,
                }
            })
            .collect();

        println!("Writing V3 database to {:?}...", output_path.as_ref());
        let out_file = File::create(output_path.as_ref())?;
        let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, out_file);

        let theme_dict_offset = std::mem::size_of::<DbHeaderV3>() as u64;
        let theme_dict_size = (theme_dict.len() * std::mem::size_of::<ThemeMask>()) as u64;

        let block_index_offset = theme_dict_offset + theme_dict_size;
        let block_index_size = (num_blocks * std::mem::size_of::<BlockEntry>()) as u64;

        let blocks_payload_offset = block_index_offset + block_index_size;

        let header = DbHeaderV3 {
            magic: *LPDB_MAGIC,
            version: LPDB_VERSION_V3,
            puzzle_count: total_puzzles as u64,
            block_count: num_blocks as u32,
            block_size: chunk_size as u32,
            block_index_offset,
            theme_dict_offset,
            theme_dict_count: unique_themes as u32,
            min_rating,
            max_rating,
            reserved: [0u8; 16],
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
            "Successfully created LPDB V3 Block-Compressed binary! Size: {:.2} MB (Uncompressed: {:.2} MB, Ratio: {:.1}%) in {:.2}s",
            total_bytes_written as f64 / (1024.0 * 1024.0),
            total_uncompressed_bytes as f64 / (1024.0 * 1024.0),
            (total_bytes_written as f64 / total_uncompressed_bytes as f64) * 100.0,
            duration_secs
        );

        Ok(BlockBuilderStats {
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
