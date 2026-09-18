//! Columnar block compression (Format V4).
//!
//! Provides ultra-compact columnar layout with adaptive fallback mechanisms,
//! delta-encoded ratings, contiguous piece and move streams, and fast LRU block caching.

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
pub const LPDB_VERSION_V4: u32 = 4;
pub const DEFAULT_V4_BLOCK_SIZE: usize = 2048;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DbHeaderV4 {
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
    pub compression_level: u8,
    pub reserved: [u8; 15],
}

const _: () = assert!(std::mem::size_of::<DbHeaderV4>() == 64);

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct BlockEntryV4 {
    pub file_offset: u64,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub puzzle_count: u16,
    pub min_rating: u16,
    pub max_rating: u16,
    pub flags: u16,
}

const _: () = assert!(std::mem::size_of::<BlockEntryV4>() == 24);

/// Decompressed Columnar Block Representation in Memory
pub struct DecompressedColumnarBlock {
    pub block_idx: usize,
    pub puzzle_count: usize,
    pub ids: Vec<u32>,
    pub overflow_ids: Vec<String>,
    pub ratings: Vec<u16>,
    pub theme_ids: Vec<u32>,
    pub fen_states: Vec<u16>,
    pub fen_halfmoves: Vec<u8>,
    pub board_occupancies: Vec<u64>,
    pub piece_streams: Vec<Vec<u8>>,
    pub move_streams: Vec<Vec<PackedMove>>,
}

impl DecompressedColumnarBlock {
    /// Decodes a decompressed columnar block payload from raw bytes.
    pub fn decode_from_bytes(block_idx: usize, data: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(data);
        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        if count == 0 {
            return Ok(Self {
                block_idx,
                puzzle_count: 0,
                ids: Vec::new(),
                overflow_ids: Vec::new(),
                ratings: Vec::new(),
                theme_ids: Vec::new(),
                fen_states: Vec::new(),
                fen_halfmoves: Vec::new(),
                board_occupancies: Vec::new(),
                piece_streams: Vec::new(),
                move_streams: Vec::new(),
            });
        }

        // 1. IDs column (count * 4 bytes)
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 4];
            cursor.read_exact(&mut buf)?;
            ids.push(u32::from_le_bytes(buf));
        }

        // 2. Ratings column (delta-encoded)
        let mut ratings = Vec::with_capacity(count);
        let mut first_r_buf = [0u8; 2];
        cursor.read_exact(&mut first_r_buf)?;
        let first_r = u16::from_le_bytes(first_r_buf);
        ratings.push(first_r);
        let mut current_r = first_r;

        for _ in 1..count {
            let mut delta_buf = [0u8; 2];
            cursor.read_exact(&mut delta_buf)?;
            let delta = u16::from_le_bytes(delta_buf);
            current_r = current_r.wrapping_add(delta);
            ratings.push(current_r);
        }

        // 3. Theme IDs column (count * 4 bytes)
        let mut theme_ids = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 4];
            cursor.read_exact(&mut buf)?;
            theme_ids.push(u32::from_le_bytes(buf));
        }

        // 4. FEN states & halfmoves (count * 3 bytes)
        let mut fen_states = Vec::with_capacity(count);
        let mut fen_halfmoves = Vec::with_capacity(count);
        for _ in 0..count {
            let mut state_buf = [0u8; 2];
            cursor.read_exact(&mut state_buf)?;
            fen_states.push(u16::from_le_bytes(state_buf));

            let mut hm_buf = [0u8; 1];
            cursor.read_exact(&mut hm_buf)?;
            fen_halfmoves.push(hm_buf[0]);
        }

        // 5. Board Occupancies (count * 8 bytes)
        let mut board_occupancies = Vec::with_capacity(count);
        for _ in 0..count {
            let mut occ_buf = [0u8; 8];
            cursor.read_exact(&mut occ_buf)?;
            board_occupancies.push(u64::from_le_bytes(occ_buf));
        }

        // 6. Piece streams
        let mut piece_lens = vec![0u8; count];
        cursor.read_exact(&mut piece_lens)?;
        let mut piece_streams = Vec::with_capacity(count);
        for len in piece_lens {
            let mut piece_buf = vec![0u8; len as usize];
            cursor.read_exact(&mut piece_buf)?;
            piece_streams.push(piece_buf);
        }

        // 7. Move streams
        let mut move_counts = vec![0u8; count];
        cursor.read_exact(&mut move_counts)?;
        let mut move_streams = Vec::with_capacity(count);
        for m_count in move_counts {
            let total_bytes = (m_count as usize) * 2;
            let mut move_bytes = vec![0u8; total_bytes];
            cursor.read_exact(&mut move_bytes)?;

            let mut moves = Vec::with_capacity(m_count as usize);
            for chunk in move_bytes.chunks_exact(2) {
                moves.push(PackedMove(u16::from_le_bytes([chunk[0], chunk[1]])));
            }
            move_streams.push(moves);
        }

        // 8. Overflow string pool (adaptive fallback)
        let mut overflow_ids = Vec::new();
        let mut overflow_count_buf = [0u8; 2];
        if cursor.read_exact(&mut overflow_count_buf).is_ok() {
            let overflow_count = u16::from_le_bytes(overflow_count_buf) as usize;
            for _ in 0..overflow_count {
                let mut str_len_buf = [0u8; 1];
                cursor.read_exact(&mut str_len_buf)?;
                let mut str_bytes = vec![0u8; str_len_buf[0] as usize];
                cursor.read_exact(&mut str_bytes)?;
                overflow_ids.push(String::from_utf8_lossy(&str_bytes).into_owned());
            }
        }

        Ok(Self {
            block_idx,
            puzzle_count: count,
            ids,
            overflow_ids,
            ratings,
            theme_ids,
            fen_states,
            fen_halfmoves,
            board_occupancies,
            piece_streams,
            move_streams,
        })
    }

    /// Reconstructs a full decoded Puzzle struct for a given index in this block.
    pub fn get_puzzle(&self, index_in_block: usize, theme_dict: &[ThemeMask]) -> Option<crate::db::Puzzle> {
        if index_in_block >= self.puzzle_count {
            return None;
        }

        let raw_id = self.ids[index_in_block];
        let id = if (raw_id & 0x8000_0000) != 0 {
            let overflow_idx = (raw_id & 0x7FFF_FFFF) as usize;
            self.overflow_ids.get(overflow_idx).cloned().unwrap_or_default()
        } else {
            decode_base62_id(raw_id)
        };

        let rating = self.ratings[index_in_block];
        let t_id = self.theme_ids[index_in_block] as usize;
        let fen_state = self.fen_states[index_in_block];
        let fen_halfmove = self.fen_halfmoves[index_in_block];
        let occ = self.board_occupancies[index_in_block];
        let piece_bytes = &self.piece_streams[index_in_block];

        let mut piece_arr = [0u8; 16];
        let copy_len = piece_bytes.len().min(16);
        piece_arr[..copy_len].copy_from_slice(&piece_bytes[..copy_len]);

        let compact_board = CompactBoard {
            occupied: occ,
            pieces: piece_arr,
        };

        // Reconstruct FEN
        let mut fen = String::with_capacity(90);
        compact_board.write_fen_ranks(&mut fen);

        fen.push(' ');
        if (fen_state & 1) == 0 {
            fen.push('w');
        } else {
            fen.push('b');
        }

        fen.push(' ');
        let castling = (fen_state >> 1) & 0x0F;
        if castling == 0 {
            fen.push('-');
        } else {
            if (castling & (1 << 0)) != 0 { fen.push('K'); }
            if (castling & (1 << 1)) != 0 { fen.push('Q'); }
            if (castling & (1 << 2)) != 0 { fen.push('k'); }
            if (castling & (1 << 3)) != 0 { fen.push('q'); }
        }

        fen.push(' ');
        let ep = (fen_state >> 5) & 0x7F;
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
        fen.push_str(&fen_halfmove.to_string());
        fen.push_str(" 1");

        let moves = self.move_streams[index_in_block].iter().map(|m| m.to_uci()).collect();

        let themes = if t_id < theme_dict.len() {
            theme_dict[t_id].to_theme_names().into_iter().map(String::from).collect()
        } else {
            Vec::new()
        };

        Some(crate::db::Puzzle {
            id,
            fen,
            moves,
            rating,
            themes,
        })
    }
}

/// Thread-safe LRU Block Cache
struct BlockLruCache {
    entries: Vec<DecompressedColumnarBlock>,
    capacity: usize,
}

impl BlockLruCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
        }
    }

    fn get(&mut self, block_idx: usize) -> Option<&DecompressedColumnarBlock> {
        if let Some(pos) = self.entries.iter().position(|b| b.block_idx == block_idx) {
            let block = self.entries.remove(pos);
            self.entries.insert(0, block);
            Some(&self.entries[0])
        } else {
            None
        }
    }

    fn insert(&mut self, block: DecompressedColumnarBlock) {
        if self.entries.len() >= self.capacity {
            self.entries.pop();
        }
        self.entries.insert(0, block);
    }
}

pub struct ColumnarDb {
    mmap: Mmap,
    header: DbHeaderV4,
    theme_dict: &'static [ThemeMask],
    block_index: &'static [BlockEntryV4],
    cache: Mutex<BlockLruCache>,
}

unsafe impl Send for ColumnarDb {}
unsafe impl Sync for ColumnarDb {}

impl ColumnarDb {
    /// Opens and memory-maps a `.lpdb` V4 columnar compressed file.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < std::mem::size_of::<DbHeaderV4>() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "File too small for V4 header"));
        }

        let header: DbHeaderV4 = *bytemuck::from_bytes(&mmap[..std::mem::size_of::<DbHeaderV4>()]);

        if &header.magic != LPDB_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid LPDB magic"));
        }

        if header.version != LPDB_VERSION_V4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported LPDB version: {} (expected {})", header.version, LPDB_VERSION_V4),
            ));
        }

        // Theme Dictionary
        let theme_dict_start = header.theme_dict_offset as usize;
        let theme_dict_len = (header.theme_dict_count as usize) * std::mem::size_of::<ThemeMask>();
        let theme_dict_slice: &[ThemeMask] = bytemuck::cast_slice(&mmap[theme_dict_start..theme_dict_start + theme_dict_len]);
        let theme_dict: &'static [ThemeMask] = unsafe { std::mem::transmute(theme_dict_slice) };

        // Block Index
        let block_index_start = header.block_index_offset as usize;
        let block_index_len = (header.block_count as usize) * std::mem::size_of::<BlockEntryV4>();
        let block_index_slice: &[BlockEntryV4] = bytemuck::cast_slice(&mmap[block_index_start..block_index_start + block_index_len]);
        let block_index: &'static [BlockEntryV4] = unsafe { std::mem::transmute(block_index_slice) };

        Ok(Self {
            mmap,
            header,
            theme_dict,
            block_index,
            cache: Mutex::new(BlockLruCache::new(16)),
        })
    }

    #[inline(always)]
    pub fn header(&self) -> &DbHeaderV4 {
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

    #[inline(always)]
    pub fn block_index(&self) -> &[BlockEntryV4] {
        self.block_index
    }

    /// Decompresses and returns the block for the given block index.
    pub fn get_block(&self, block_idx: usize) -> io::Result<DecompressedColumnarBlock> {
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

        DecompressedColumnarBlock::decode_from_bytes(block_idx, &decompressed_bytes)
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
        if let Some(block) = cache.get(block_idx) {
            return block.get_puzzle(offset_in_block, self.theme_dict);
        }

        let decompressed = self.get_block(block_idx).ok()?;
        let puzzle = decompressed.get_puzzle(offset_in_block, self.theme_dict);
        cache.insert(decompressed);
        puzzle
    }

    /// Finds a puzzle by its 5-char ID string (e.g. "00008").
    pub fn find_by_id(&self, id_str: &str) -> Option<crate::db::Puzzle> {
        let trimmed = id_str.trim();
        for block_idx in 0..self.block_index.len() {
            if let Ok(block) = self.get_block(block_idx) {
                for i in 0..block.puzzle_count {
                    if let Some(p) = block.get_puzzle(i, self.theme_dict) {
                        if p.id == trimmed {
                            return Some(p);
                        }
                    }
                }
            }
        }
        None
    }

    /// Picks a random puzzle matching criteria.
    pub fn random_puzzle<R: Rng>(&self, criteria: &crate::db::QueryCriteria, rng: &mut R) -> Option<crate::db::Puzzle> {
        let min_r = criteria.min_rating.unwrap_or(self.header.min_rating);
        let max_r = criteria.max_rating.unwrap_or(self.header.max_rating);

        if min_r > max_r || self.block_index.is_empty() {
            return None;
        }

        let start_block = self.block_index.partition_point(|b| b.max_rating < min_r);
        let end_block = self.block_index.partition_point(|b| b.min_rating <= max_r);

        if start_block >= self.block_index.len() || start_block > end_block {
            return None;
        }

        let candidates = &self.block_index[start_block..end_block];
        if candidates.is_empty() {
            return None;
        }

        for _ in 0..30 {
            let rand_block_idx = start_block + rng.gen_range(0..candidates.len());
            if let Ok(block) = self.get_block(rand_block_idx) {
                let mut matching_in_block = Vec::new();
                for i in 0..block.puzzle_count {
                    let r = block.ratings[i];
                    if r >= min_r && r <= max_r {
                        let t_id = block.theme_ids[i] as usize;
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
                    return block.get_puzzle(chosen_idx, self.theme_dict);
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
                for i in 0..block.puzzle_count {
                    let r = block.ratings[i];
                    if r >= min_r && r <= max_r {
                        let t_id = block.theme_ids[i] as usize;
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


                        if let Some(p) = block.get_puzzle(i, self.theme_dict) {
                            results.push(p);
                            if results.len() >= limit {
                                return results;
                            }
                        }
                    }
                }
            }
        }

        results
    }
}

