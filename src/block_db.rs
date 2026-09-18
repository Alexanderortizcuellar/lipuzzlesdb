//! Block-compressed database format (V3).
//!
//! Features:
//! - 512 puzzles per compressed block.
//! - Self-contained blocks: PuzzleRecords + local MovePool in each block.
//! - Block index table for O(1) seekable random access.
//! - Fast LRU / slot cache for sub-microsecond repeated queries.

use std::fs::File;
use std::io::{self, Cursor, Read};
use std::path::Path;
use std::sync::Mutex;
use bytemuck::{Pod, Zeroable};
use memmap2::Mmap;
use rand::Rng;
use crate::board::CompactBoard;
use crate::id::decode_base62_id;
use crate::moves::PackedMove;
use crate::theme::ThemeMask;

pub const LPDB_MAGIC: &[u8; 4] = b"LPDB";
pub const LPDB_VERSION_V3: u32 = 3;
pub const DEFAULT_BLOCK_SIZE: usize = 512;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DbHeaderV3 {
    pub magic: [u8; 4],
    pub version: u32,
    pub puzzle_count: u64,
    pub block_count: u32,
    pub block_size: u32,
    pub block_index_offset: u64,
    pub theme_dict_offset: u64,
    pub theme_dict_count: u32,
    pub min_rating: u16,
    pub max_rating: u16,
    pub reserved: [u8; 16],
}

const _: () = assert!(std::mem::size_of::<DbHeaderV3>() == 64);

/// Entry in the seekable block index table
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct BlockEntry {
    pub file_offset: u64,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub puzzle_count: u16,
    pub min_rating: u16,
    pub max_rating: u16,
    pub reserved: u16,
}

const _: () = assert!(std::mem::size_of::<BlockEntry>() == 24);

/// Compact 36-byte Block Puzzle Record (uses 16-bit local move offset within the block)
#[repr(C, align(4))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct BlockPuzzleRecord {
    pub board_occupied: u64,
    pub board_pieces: [u8; 16],
    pub id_base62: u32,
    pub theme_id: u32,
    pub move_local_offset: u16,
    pub move_count: u8,
    pub fen_halfmove: u8,
    pub fen_state: u16,
    pub rating: u16,
}

const _: () = assert!(std::mem::size_of::<BlockPuzzleRecord>() == 40);

impl BlockPuzzleRecord {
    #[inline(always)]
    pub fn id_string(&self) -> String {
        decode_base62_id(self.id_base62)
    }

    #[inline(always)]
    pub fn compact_board(&self) -> CompactBoard {
        CompactBoard {
            occupied: self.board_occupied,
            pieces: self.board_pieces,
        }
    }

    pub fn fen_string(&self) -> String {
        let mut fen = String::with_capacity(90);
        self.compact_board().write_fen_ranks(&mut fen);

        fen.push(' ');
        if (self.fen_state & 1) == 0 {
            fen.push('w');
        } else {
            fen.push('b');
        }

        fen.push(' ');
        let castling = (self.fen_state >> 1) & 0x0F;
        if castling == 0 {
            fen.push('-');
        } else {
            if (castling & (1 << 0)) != 0 { fen.push('K'); }
            if (castling & (1 << 1)) != 0 { fen.push('Q'); }
            if (castling & (1 << 2)) != 0 { fen.push('k'); }
            if (castling & (1 << 3)) != 0 { fen.push('q'); }
        }

        fen.push(' ');
        let ep = (self.fen_state >> 5) & 0x7F;
        if ep == 0 || ep > 64 {
            fen.push('-');
        } else {
            let sq = (ep - 1) as u8;
            let rank = sq / 8;
            let file = sq % 8;
            let file_char = (b'a' + file) as char;
            let rank_char = (b'1' + (7 - rank)) as char;
            fen.push(file_char);
            fen.push(rank_char);
        }

        fen.push(' ');
        fen.push_str(&self.fen_halfmove.to_string());
        fen.push_str(" 1");

        fen
    }

    #[inline(always)]
    pub fn get_moves<'a>(&self, block_move_pool: &'a [PackedMove]) -> &'a [PackedMove] {
        let start = self.move_local_offset as usize;
        let end = start + (self.move_count as usize);
        if end <= block_move_pool.len() {
            &block_move_pool[start..end]
        } else {
            &[]
        }
    }
}

/// Decompressed Block Payload in memory
pub struct DecompressedBlock {
    pub block_idx: usize,
    pub records: Vec<BlockPuzzleRecord>,
    pub moves: Vec<PackedMove>,
}

impl DecompressedBlock {
    pub fn decode_from_bytes(block_idx: usize, data: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(data);
        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let puzzle_count = u32::from_le_bytes(count_bytes) as usize;

        cursor.read_exact(&mut count_bytes)?;
        let move_count = u32::from_le_bytes(count_bytes) as usize;

        let records_bytes_len = puzzle_count * std::mem::size_of::<BlockPuzzleRecord>();
        let pos = cursor.position() as usize;
        if pos + records_bytes_len > data.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Block records overflow"));
        }

        let records_slice: &[BlockPuzzleRecord] = bytemuck::cast_slice(&data[pos..pos + records_bytes_len]);
        let records = records_slice.to_vec();

        let moves_pos = pos + records_bytes_len;
        let moves_bytes_len = move_count * std::mem::size_of::<PackedMove>();
        if moves_pos + moves_bytes_len > data.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Block moves overflow"));
        }

        let moves_slice: &[PackedMove] = bytemuck::cast_slice(&data[moves_pos..moves_pos + moves_bytes_len]);
        let moves = moves_slice.to_vec();

        Ok(Self {
            block_idx,
            records,
            moves,
        })
    }
}

pub struct BlockCompressedDb {
    mmap: Mmap,
    header: DbHeaderV3,
    theme_dict: &'static [ThemeMask],
    block_index: &'static [BlockEntry],
    cache: Mutex<Option<DecompressedBlock>>,
}

unsafe impl Send for BlockCompressedDb {}
unsafe impl Sync for BlockCompressedDb {}

impl BlockCompressedDb {
    /// Opens and memory-maps a `.lpdb` V3 block-compressed file.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < std::mem::size_of::<DbHeaderV3>() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "File too small for V3 header"));
        }

        let header: DbHeaderV3 = *bytemuck::from_bytes(&mmap[..std::mem::size_of::<DbHeaderV3>()]);

        if &header.magic != LPDB_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid LPDB magic"));
        }

        if header.version != LPDB_VERSION_V3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported LPDB version: {} (expected {})", header.version, LPDB_VERSION_V3),
            ));
        }

        // Theme Dictionary
        let theme_dict_start = header.theme_dict_offset as usize;
        let theme_dict_len = (header.theme_dict_count as usize) * std::mem::size_of::<ThemeMask>();
        let theme_dict_slice: &[ThemeMask] = bytemuck::cast_slice(&mmap[theme_dict_start..theme_dict_start + theme_dict_len]);
        let theme_dict: &'static [ThemeMask] = unsafe { std::mem::transmute(theme_dict_slice) };

        // Block Index
        let block_index_start = header.block_index_offset as usize;
        let block_index_len = (header.block_count as usize) * std::mem::size_of::<BlockEntry>();
        let block_index_slice: &[BlockEntry] = bytemuck::cast_slice(&mmap[block_index_start..block_index_start + block_index_len]);
        let block_index: &'static [BlockEntry] = unsafe { std::mem::transmute(block_index_slice) };

        Ok(Self {
            mmap,
            header,
            theme_dict,
            block_index,
            cache: Mutex::new(None),
        })
    }

    #[inline(always)]
    pub fn header(&self) -> &DbHeaderV3 {
        &self.header
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.header.puzzle_count as usize
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    pub fn theme_dict(&self) -> &[ThemeMask] {
        self.theme_dict
    }

    /// Decompresses and returns the block for the given block index.
    pub fn get_block(&self, block_idx: usize) -> io::Result<DecompressedBlock> {
        if block_idx >= self.block_index.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Block index out of bounds"));
        }

        let entry = &self.block_index[block_idx];
        let offset = entry.file_offset as usize;
        let comp_size = entry.compressed_size as usize;
        let uncomp_size = entry.uncompressed_size as usize;

        let compressed_bytes = &self.mmap[offset..offset + comp_size];
        let decompressed_bytes = zstd::decode_all(compressed_bytes)?;

        if decompressed_bytes.len() != uncomp_size {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Decompressed size mismatch"));
        }

        DecompressedBlock::decode_from_bytes(block_idx, &decompressed_bytes)
    }

    /// Fetches a decoded puzzle by index.
    pub fn get(&self, puzzle_idx: usize) -> Option<crate::db::Puzzle> {
        if puzzle_idx >= self.len() {
            return None;
        }

        let block_size = self.header.block_size as usize;
        let block_idx = puzzle_idx / block_size;
        let offset_in_block = puzzle_idx % block_size;

        let mut cache = self.cache.lock().unwrap();
        let hit = cache.as_ref().map_or(false, |b| b.block_idx == block_idx);

        if !hit {
            let decompressed = self.get_block(block_idx).ok()?;
            *cache = Some(decompressed);
        }

        let block = cache.as_ref().unwrap();
        let record = block.records.get(offset_in_block)?;
        let moves = record.get_moves(&block.moves).iter().map(|m| m.to_uci()).collect();

        let t_id = record.theme_id as usize;
        let themes = if t_id < self.theme_dict.len() {
            self.theme_dict[t_id].to_theme_names().into_iter().map(String::from).collect()
        } else {
            Vec::new()
        };

        Some(crate::db::Puzzle {
            id: record.id_string(),
            fen: record.fen_string(),
            moves,
            rating: record.rating,
            themes,
        })
    }

    /// Picks a random puzzle matching rating and theme criteria.
    pub fn random_puzzle<R: Rng>(&self, criteria: &crate::db::QueryCriteria, rng: &mut R) -> Option<crate::db::Puzzle> {
        let min_r = criteria.min_rating.unwrap_or(self.header.min_rating);
        let max_r = criteria.max_rating.unwrap_or(self.header.max_rating);

        if min_r > max_r || self.block_index.is_empty() {
            return None;
        }

        // Find matching block range (since blocks are sorted by rating)
        let start_block = self.block_index.partition_point(|b| b.max_rating < min_r);
        let end_block = self.block_index.partition_point(|b| b.min_rating <= max_r);

        if start_block >= self.block_index.len() || start_block > end_block {
            return None;
        }

        let candidates = &self.block_index[start_block..end_block];
        if candidates.is_empty() {
            return None;
        }

        // Try up to 20 random samples from matching blocks
        for _ in 0..20 {
            let rand_block_idx = start_block + rng.gen_range(0..candidates.len());
            if let Ok(block) = self.get_block(rand_block_idx) {
                let mut matching_in_block = Vec::new();
                for (i, r) in block.records.iter().enumerate() {
                    if r.rating >= min_r && r.rating <= max_r {
                        let t_id = r.theme_id as usize;
                        let mask = if t_id < self.theme_dict.len() { self.theme_dict[t_id] } else { ThemeMask::EMPTY };

                        if let Some(req) = criteria.required_themes {
                            if !mask.contains_all(req) {
                                continue;
                            }
                        }
                        if let Some(any) = criteria.any_themes {
                            if !mask.contains_any(any) {
                                continue;
                            }
                        }
                        if let Some(excl) = criteria.excluded_themes {
                            if mask.contains_any(excl) {
                                continue;
                            }
                        }
                        matching_in_block.push(i);

                    }
                }

                if !matching_in_block.is_empty() {
                    let chosen_idx = matching_in_block[rng.gen_range(0..matching_in_block.len())];
                    let r = &block.records[chosen_idx];
                    let moves = r.get_moves(&block.moves).iter().map(|m| m.to_uci()).collect();
                    let t_id = r.theme_id as usize;
                    let themes = if t_id < self.theme_dict.len() {
                        self.theme_dict[t_id].to_theme_names().into_iter().map(String::from).collect()
                    } else {
                        Vec::new()
                    };

                    return Some(crate::db::Puzzle {
                        id: r.id_string(),
                        fen: r.fen_string(),
                        moves,
                        rating: r.rating,
                        themes,
                    });
                }
            }
        }

        None
    }

    /// Queries puzzles matching criteria up to a given limit.
    pub fn query_puzzles(&self, criteria: &crate::db::QueryCriteria, limit: usize) -> Vec<crate::db::Puzzle> {
        let min_r = criteria.min_rating.unwrap_or(self.header.min_rating);
        let max_r = criteria.max_rating.unwrap_or(self.header.max_rating);

        if min_r > max_r || self.block_index.is_empty() || limit == 0 {
            return Vec::new();
        }

        let start_block = self.block_index.partition_point(|b| b.max_rating < min_r);
        let end_block = self.block_index.partition_point(|b| b.min_rating <= max_r);

        let mut results = Vec::with_capacity(limit.min(100_000));

        for block_idx in start_block..end_block.min(self.block_index.len()) {
            if let Ok(block) = self.get_block(block_idx) {
                for (i, r) in block.records.iter().enumerate() {
                    if r.rating >= min_r && r.rating <= max_r {
                        let t_id = r.theme_id as usize;
                        let mask = if t_id < self.theme_dict.len() { self.theme_dict[t_id] } else { ThemeMask::EMPTY };

                        if let Some(req) = criteria.required_themes {
                            if !mask.contains_all(req) {
                                continue;
                            }
                        }
                        if let Some(any) = criteria.any_themes {
                            if !mask.contains_any(any) {
                                continue;
                            }
                        }
                        if let Some(excl) = criteria.excluded_themes {
                            if mask.contains_any(excl) {
                                continue;
                            }
                        }


                        let moves = r.get_moves(&block.moves).iter().map(|m| m.to_uci()).collect();
                        let themes = if t_id < self.theme_dict.len() {
                            self.theme_dict[t_id].to_theme_names().into_iter().map(String::from).collect()
                        } else {
                            Vec::new()
                        };

                        results.push(crate::db::Puzzle {
                            id: r.id_string(),
                            fen: r.fen_string(),
                            moves,
                            rating: r.rating,
                            themes,
                        });

                        if results.len() >= limit {
                            return results;
                        }
                    }
                }
            }
        }

        results
    }
}

