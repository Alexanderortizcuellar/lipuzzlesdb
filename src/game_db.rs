//! Ultra-compact companion database format (.lgdb) for storing full game moves.
//!
//! Binary Layout:
//! - 64-byte File Header (Magic b"LGDB", Version 1, game count, offsets, sizes)
//! - Index Table: Array of 24-byte `GameIndexEntry` structs
//! - Data Block: Zstandard-compressed array of 16-bit `PackedMove`s

use bytemuck::{Pod, Zeroable};
use memmap2::Mmap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::id::{decode_base62_id, encode_base62_id, is_standard_base62_5char};
use crate::moves::PackedMove;

pub const LGDB_MAGIC: &[u8; 4] = b"LGDB";
pub const LGDB_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameResult {
    Unknown,
    WhiteWin,
    BlackWin,
    Draw,
}

impl GameResult {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => GameResult::WhiteWin,
            2 => GameResult::BlackWin,
            3 => GameResult::Draw,
            _ => GameResult::Unknown,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            GameResult::Unknown => 0,
            GameResult::WhiteWin => 1,
            GameResult::BlackWin => 2,
            GameResult::Draw => 3,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GameResult::Unknown => "*",
            GameResult::WhiteWin => "1-0",
            GameResult::BlackWin => "0-1",
            GameResult::Draw => "1/2-1/2",
        }
    }

    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        if trimmed == "1-0" || trimmed.eq_ignore_ascii_case("white") {
            GameResult::WhiteWin
        } else if trimmed == "0-1" || trimmed.eq_ignore_ascii_case("black") {
            GameResult::BlackWin
        } else if trimmed == "1/2-1/2" || trimmed.eq_ignore_ascii_case("draw") {
            GameResult::Draw
        } else {
            GameResult::Unknown
        }
    }
}

/// 64-byte Header for `.lgdb` files.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct LgdbHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub game_count: u32,
    pub reserved_head: u32,
    pub index_offset: u64,
    pub data_offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub reserved: [u8; 16],
}

const _: () = assert!(std::mem::size_of::<LgdbHeader>() == 64);
const _: () = assert!(std::mem::align_of::<LgdbHeader>() == 8);

/// 24-byte Index Entry per game.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct GameIndexEntry {
    /// Base62-encoded 5-char puzzle ID (compatible with PuzzleRecord.id_base62)
    pub puzzle_id: u32,
    /// Offset in the decompressed move pool (in PackedMove units)
    pub move_offset: u32,
    /// Up to 8 ASCII characters for the Lichess Game ID (e.g. "KnvUoTQB")
    pub game_id: [u8; 8],
    /// Packed date: (YYYY << 16) | (MM << 8) | DD
    pub date_packed: u32,
    /// Number of plies in the game
    pub ply_count: u16,
    /// Game result (0 = *, 1 = 1-0, 2 = 0-1, 3 = 1/2-1/2)
    pub result: u8,
    /// Alignment padding
    pub reserved: u8,
}

const _: () = assert!(std::mem::size_of::<GameIndexEntry>() == 24);
const _: () = assert!(std::mem::align_of::<GameIndexEntry>() == 8);

impl GameIndexEntry {
    pub fn puzzle_id_str(&self) -> String {
        decode_base62_id(self.puzzle_id)
    }

    pub fn game_id_str(&self) -> String {
        let len = self.game_id.iter().position(|&b| b == 0).unwrap_or(8);
        String::from_utf8_lossy(&self.game_id[..len]).to_string()
    }

    pub fn date_ymd(&self) -> Option<(u16, u8, u8)> {
        if self.date_packed == 0 {
            return None;
        }
        let year = (self.date_packed >> 16) as u16;
        let month = ((self.date_packed >> 8) & 0xFF) as u8;
        let day = (self.date_packed & 0xFF) as u8;
        Some((year, month, day))
    }

    pub fn date_str(&self) -> String {
        if let Some((y, m, d)) = self.date_ymd() {
            format!("{:04}.{:02}.{:02}", y, m, d)
        } else {
            "????.??.??".to_string()
        }
    }

    pub fn game_result(&self) -> GameResult {
        GameResult::from_u8(self.result)
    }
}

pub struct GameRecord<'a> {
    pub puzzle_id: String,
    pub game_id: String,
    pub result: GameResult,
    pub date: String,
    pub moves: &'a [PackedMove],
}

pub struct Lgdb {
    _mmap: Option<Mmap>,
    header: LgdbHeader,
    entries: Vec<GameIndexEntry>,
    move_pool: Vec<PackedMove>,
}

impl Lgdb {
    /// Opens an `.lgdb` file and loads the game database into memory.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < std::mem::size_of::<LgdbHeader>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File too small for LGDB header",
            ));
        }

        let header_bytes = &mmap[..std::mem::size_of::<LgdbHeader>()];
        let header: LgdbHeader = *bytemuck::from_bytes(header_bytes);

        if &header.magic != LGDB_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid LGDB magic header",
            ));
        }

        let game_count = header.game_count as usize;
        let index_start = header.index_offset as usize;
        let index_end = index_start + game_count * std::mem::size_of::<GameIndexEntry>();

        if index_end > mmap.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Index table truncated",
            ));
        }

        let index_slice = &mmap[index_start..index_end];
        let entries_slice: &[GameIndexEntry] = bytemuck::cast_slice(index_slice);
        let entries = entries_slice.to_vec();

        let data_start = header.data_offset as usize;
        let data_end = data_start + header.compressed_size as usize;
        if data_end > mmap.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Compressed data block truncated",
            ));
        }

        let compressed_data = &mmap[data_start..data_end];
        let decompressed_bytes = zstd::decode_all(compressed_data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Zstd decompression failed: {}", e),
            )
        })?;

        let move_pool: Vec<PackedMove> = bytemuck::cast_slice(&decompressed_bytes).to_vec();

        Ok(Self {
            _mmap: Some(mmap),
            header,
            entries,
            move_pool,
        })
    }

    pub fn header(&self) -> &LgdbHeader {
        &self.header
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[GameIndexEntry] {
        &self.entries
    }

    pub fn get(&self, idx: usize) -> Option<GameRecord<'_>> {
        let entry = self.entries.get(idx)?;
        let start = entry.move_offset as usize;
        let end = start + entry.ply_count as usize;
        let moves = if end <= self.move_pool.len() {
            &self.move_pool[start..end]
        } else {
            &[]
        };

        Some(GameRecord {
            puzzle_id: entry.puzzle_id_str(),
            game_id: entry.game_id_str(),
            result: entry.game_result(),
            date: entry.date_str(),
            moves,
        })
    }

    /// Finds game by puzzle ID (e.g. "00tbH").
    pub fn find_by_puzzle_id(&self, puzzle_id: &str) -> Option<GameRecord<'_>> {
        let target_base62 = if is_standard_base62_5char(puzzle_id) {
            encode_base62_id(puzzle_id)
        } else {
            return None;
        };

        let idx = self
            .entries
            .iter()
            .position(|e| e.puzzle_id == target_base62)?;
        self.get(idx)
    }

    /// Finds game by 8-character Lichess Game ID (e.g. "KnvUoTQB").
    pub fn find_by_game_id(&self, game_id: &str) -> Option<GameRecord<'_>> {
        let clean = game_id.trim();
        let idx = self
            .entries
            .iter()
            .position(|e| e.game_id_str().eq_ignore_ascii_case(clean))?;
        self.get(idx)
    }

    /// Reconstructs a full PGN string from a game index.
    pub fn to_pgn(&self, idx: usize) -> Option<String> {
        let game = self.get(idx)?;
        let mut pgn = String::with_capacity(1024);

        pgn.push_str("[Event \"Lichess Game\"]\n");
        if !game.game_id.is_empty() {
            pgn.push_str(&format!(
                "[Site \"https://lichess.org/{}\"]\n",
                game.game_id
            ));
            pgn.push_str(&format!("[GameId \"{}\"]\n", game.game_id));
        }
        if !game.puzzle_id.is_empty() {
            pgn.push_str(&format!("[PuzzleId \"{}\"]\n", game.puzzle_id));
        }
        pgn.push_str(&format!("[Date \"{}\"]\n", game.date));
        pgn.push_str("[White \"White\"]\n");
        pgn.push_str("[Black \"Black\"]\n");
        pgn.push_str(&format!("[Result \"{}\"]\n\n", game.result.as_str()));

        // Format moves with fullmove numbers
        for (ply_idx, m) in game.moves.iter().enumerate() {
            if ply_idx % 2 == 0 {
                let fullmove = (ply_idx / 2) + 1;
                pgn.push_str(&format!("{}. ", fullmove));
            }
            pgn.push_str(&m.to_uci());
            pgn.push(' ');
        }
        pgn.push_str(game.result.as_str());
        pgn.push('\n');

        Some(pgn)
    }
}

pub struct LgdbStats {
    pub game_count: usize,
    pub total_plies: usize,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub total_file_size: u64,
    pub compression_ratio: f64,
}

#[derive(Default)]
pub struct LgdbBuilder {
    entries: Vec<GameIndexEntry>,
    move_pool: Vec<PackedMove>,
}

impl LgdbBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn total_plies(&self) -> usize {
        self.move_pool.len()
    }

    #[inline(always)]
    pub fn game_count(&self) -> usize {
        self.entries.len()
    }

    /// Parses date string "YYYY.MM.DD" or "YYYY-MM-DD" into packed u32.
    pub fn pack_date(date_str: &str) -> u32 {
        let cleaned = date_str.replace(['/', '-'], ".");
        let parts: Vec<&str> = cleaned.split('.').collect();
        if parts.len() == 3 {
            let y = parts[0].parse::<u16>().unwrap_or(0);
            let m = parts[1].parse::<u8>().unwrap_or(0);
            let d = parts[2].parse::<u8>().unwrap_or(0);
            ((y as u32) << 16) | ((m as u32) << 8) | (d as u32)
        } else {
            0
        }
    }

    /// Adds a game to the builder.
    pub fn add_game(
        &mut self,
        puzzle_id: &str,
        game_id: &str,
        date_str: Option<&str>,
        result_str: &str,
        moves: &[PackedMove],
    ) {
        let puzzle_base62 = if is_standard_base62_5char(puzzle_id) {
            encode_base62_id(puzzle_id)
        } else {
            0
        };

        let mut gid = [0u8; 8];
        let clean_gid = game_id.trim();
        let bytes_to_copy = clean_gid.len().min(8);
        gid[..bytes_to_copy].copy_from_slice(&clean_gid.as_bytes()[..bytes_to_copy]);

        let date_packed = date_str.map(Self::pack_date).unwrap_or(0);
        let result = GameResult::parse(result_str).to_u8();

        let move_offset = self.move_pool.len() as u32;
        let ply_count = moves.len().min(u16::MAX as usize) as u16;

        self.move_pool.extend_from_slice(moves);

        self.entries.push(GameIndexEntry {
            puzzle_id: puzzle_base62,
            move_offset,
            game_id: gid,
            date_packed,
            ply_count,
            result,
            reserved: 0,
        });
    }

    /// Compresses and writes the `.lgdb` file.
    pub fn write_to_file(
        &self,
        path: impl AsRef<Path>,
        compression_level: i32,
    ) -> io::Result<LgdbStats> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        let uncompressed_moves_bytes = bytemuck::cast_slice::<PackedMove, u8>(&self.move_pool);
        let uncompressed_size = uncompressed_moves_bytes.len() as u64;

        let compressed_moves = zstd::encode_all(uncompressed_moves_bytes, compression_level)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Zstd compression failed: {}", e),
                )
            })?;
        let compressed_size = compressed_moves.len() as u64;

        let header_size = std::mem::size_of::<LgdbHeader>() as u64;
        let index_size = (self.entries.len() * std::mem::size_of::<GameIndexEntry>()) as u64;

        let index_offset = header_size;
        let data_offset = header_size + index_size;

        let header = LgdbHeader {
            magic: *LGDB_MAGIC,
            version: LGDB_VERSION,
            game_count: self.entries.len() as u32,
            reserved_head: 0,
            index_offset,
            data_offset,
            compressed_size,
            uncompressed_size,
            reserved: [0u8; 16],
        };

        // 1. Write Header
        writer.write_all(bytemuck::bytes_of(&header))?;

        // 2. Write Index Table
        let index_bytes = bytemuck::cast_slice::<GameIndexEntry, u8>(&self.entries);
        writer.write_all(index_bytes)?;

        // 3. Write Compressed Data
        writer.write_all(&compressed_moves)?;
        writer.flush()?;

        let total_file_size = header_size + index_size + compressed_size;
        let ratio = if uncompressed_size > 0 {
            total_file_size as f64 / (uncompressed_size + index_size) as f64
        } else {
            1.0
        };

        Ok(LgdbStats {
            game_count: self.entries.len(),
            total_plies: self.move_pool.len(),
            uncompressed_size,
            compressed_size,
            total_file_size,
            compression_ratio: ratio,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lgdb_roundtrip() {
        let mut builder = LgdbBuilder::new();
        let m1 = [PackedMove::new(12, 28, 0), PackedMove::new(52, 36, 0)]; // e2e4, e7e5
        let m2 = [PackedMove::new(11, 27, 0), PackedMove::new(51, 35, 0)]; // d2d4, d7d5

        builder.add_game("00tbH", "KnvUoTQB", Some("2025.06.20"), "1-0", &m1);
        builder.add_game("0mlEe", "abc12345", Some("2026.01.15"), "0-1", &m2);

        let temp_path = std::env::temp_dir().join("test_roundtrip.lgdb");
        let stats = builder.write_to_file(&temp_path, 3).expect("write failed");
        assert_eq!(stats.game_count, 2);
        assert_eq!(stats.total_plies, 4);

        let lgdb = Lgdb::open(&temp_path).expect("open failed");
        assert_eq!(lgdb.len(), 2);

        let g0 = lgdb.get(0).unwrap();
        assert_eq!(g0.puzzle_id, "00tbH");
        assert_eq!(g0.game_id, "KnvUoTQB");
        assert_eq!(g0.result, GameResult::WhiteWin);
        assert_eq!(g0.date, "2025.06.20");
        assert_eq!(g0.moves.len(), 2);
        assert_eq!(g0.moves[0].to_uci(), "e2e4");
        assert_eq!(g0.moves[1].to_uci(), "e7e5");

        let found = lgdb.find_by_puzzle_id("00tbH").unwrap();
        assert_eq!(found.game_id, "KnvUoTQB");

        let found_g = lgdb.find_by_game_id("abc12345").unwrap();
        assert_eq!(found_g.puzzle_id, "0mlEe");
        assert_eq!(found_g.result, GameResult::BlackWin);

        let _ = std::fs::remove_file(temp_path);
    }
}
