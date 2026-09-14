//! Memory-mapped zero-copy database reader and query engine (V2 format).

use std::fs::File;
use std::io;
use std::path::Path;
use bytemuck::{Pod, Zeroable};
use memmap2::Mmap;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::id::encode_base62_id;
use crate::moves::PackedMove;
use crate::record::PuzzleRecord;
use crate::theme::ThemeMask;

pub const LPDB_MAGIC: &[u8; 4] = b"LPDB";
pub const LPDB_VERSION: u32 = 2;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DbHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub puzzle_count: u64,
    pub records_offset: u64,
    pub move_pool_count: u64,
    pub move_pool_offset: u64,
    pub theme_dict_offset: u64,
    pub theme_dict_count: u32,
    pub min_rating: u16,
    pub max_rating: u16,
    pub reserved: [u8; 8],
}

const _: () = assert!(std::mem::size_of::<DbHeader>() == 64);

/// High-level decoded Puzzle view
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Puzzle {
    pub id: String,
    pub fen: String,
    pub moves: Vec<String>,
    pub rating: u16,
    pub themes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct QueryCriteria {
    pub min_rating: Option<u16>,
    pub max_rating: Option<u16>,
    pub required_themes: Option<ThemeMask>,
    pub any_themes: Option<ThemeMask>,
}

pub struct PuzzleDatabase {
    _mmap: Mmap,
    header: DbHeader,
    records: &'static [PuzzleRecord],
    move_pool: &'static [PackedMove],
    theme_dict: &'static [ThemeMask],
}

unsafe impl Send for PuzzleDatabase {}
unsafe impl Sync for PuzzleDatabase {}

impl PuzzleDatabase {
    /// Opens and memory-maps a `.lpdb` binary database file.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        if mmap.len() < std::mem::size_of::<DbHeader>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File too small for LPDB header",
            ));
        }

        let header: DbHeader = *bytemuck::from_bytes(&mmap[..std::mem::size_of::<DbHeader>()]);

        if &header.magic != LPDB_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid LPDB magic header",
            ));
        }

        if header.version != LPDB_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported LPDB version: {} (expected {})", header.version, LPDB_VERSION),
            ));
        }

        // 1. Theme Dictionary
        let theme_dict_start = header.theme_dict_offset as usize;
        let theme_dict_bytes_len = (header.theme_dict_count as usize) * std::mem::size_of::<ThemeMask>();
        let theme_dict_end = theme_dict_start + theme_dict_bytes_len;

        if theme_dict_end > mmap.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Theme dictionary section exceeds file size"));
        }

        // 2. Records
        let records_start = header.records_offset as usize;
        let records_bytes_len = (header.puzzle_count as usize) * std::mem::size_of::<PuzzleRecord>();
        let records_end = records_start + records_bytes_len;

        if records_end > mmap.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Records section exceeds file size"));
        }

        // 3. Move Pool
        let move_pool_start = header.move_pool_offset as usize;
        let move_pool_bytes_len = (header.move_pool_count as usize) * std::mem::size_of::<PackedMove>();
        let move_pool_end = move_pool_start + move_pool_bytes_len;

        if move_pool_end > mmap.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Move pool section exceeds file size"));
        }

        let theme_dict_slice: &[ThemeMask] = bytemuck::cast_slice(&mmap[theme_dict_start..theme_dict_end]);
        let records_slice: &[PuzzleRecord] = bytemuck::cast_slice(&mmap[records_start..records_end]);
        let move_pool_slice: &[PackedMove] = bytemuck::cast_slice(&mmap[move_pool_start..move_pool_end]);

        let theme_dict: &'static [ThemeMask] = unsafe { std::mem::transmute(theme_dict_slice) };
        let records: &'static [PuzzleRecord] = unsafe { std::mem::transmute(records_slice) };
        let move_pool: &'static [PackedMove] = unsafe { std::mem::transmute(move_pool_slice) };

        Ok(Self {
            _mmap: mmap,
            header,
            records,
            move_pool,
            theme_dict,
        })
    }

    #[inline(always)]
    pub fn header(&self) -> &DbHeader {
        &self.header
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[inline(always)]
    pub fn records(&self) -> &[PuzzleRecord] {
        self.records
    }

    #[inline(always)]
    pub fn move_pool(&self) -> &[PackedMove] {
        self.move_pool
    }

    #[inline(always)]
    pub fn theme_dict(&self) -> &[ThemeMask] {
        self.theme_dict
    }

    /// Fetches a high-level decoded Puzzle by index.
    pub fn get(&self, index: usize) -> Option<Puzzle> {
        let record = self.records.get(index)?;
        let moves_packed = record.get_moves(self.move_pool);
        let moves = moves_packed.iter().map(|m| m.to_uci()).collect();
        let t_id = record.theme_id() as usize;
        let themes = if t_id < self.theme_dict.len() {
            self.theme_dict[t_id].to_theme_names().into_iter().map(String::from).collect()
        } else {
            Vec::new()
        };

        Some(Puzzle {
            id: record.id_string(),
            fen: record.fen_string(),
            moves,
            rating: record.rating(),
            themes,
        })
    }

    /// Finds a puzzle by its ID (e.g. "00008").
    pub fn find_by_id(&self, id_str: &str) -> Option<Puzzle> {
        let trimmed = id_str.trim();
        let target_id = encode_base62_id(trimmed);
        self.records
            .iter()
            .position(|r| r.id_base62 == target_id)
            .and_then(|idx| self.get(idx))
    }

    /// Returns a slice of records matching a rating range [min_rating, max_rating] (inclusive).
    /// Assumes records are sorted by rating.
    pub fn rating_range(&self, min_rating: u16, max_rating: u16) -> &[PuzzleRecord] {
        if self.records.is_empty() || min_rating > max_rating {
            return &[];
        }

        let start_idx = self.records.partition_point(|r| r.rating() < min_rating);
        let end_idx = self.records.partition_point(|r| r.rating() <= max_rating);

        if start_idx < self.records.len() && start_idx <= end_idx {
            &self.records[start_idx..end_idx]
        } else {
            &[]
        }
    }

    /// Filters puzzles matching criteria.
    pub fn filter<'a>(&'a self, criteria: &'a QueryCriteria) -> impl Iterator<Item = &'a PuzzleRecord> + 'a {
        let base_slice = match (criteria.min_rating, criteria.max_rating) {
            (Some(min_r), Some(max_r)) => self.rating_range(min_r, max_r),
            (Some(min_r), None) => self.rating_range(min_r, u16::MAX),
            (None, Some(max_r)) => self.rating_range(0, max_r),
            (None, None) => self.records,
        };

        base_slice.iter().filter(move |r| {
            if let Some(req) = criteria.required_themes {
                if !r.has_themes_all(req, self.theme_dict) {
                    return false;
                }
            }
            if let Some(any) = criteria.any_themes {
                if !r.has_themes_any(any, self.theme_dict) {
                    return false;
                }
            }
            true
        })
    }

    /// Selects a random puzzle matching criteria.
    pub fn random_puzzle<R: Rng>(&self, criteria: &QueryCriteria, rng: &mut R) -> Option<Puzzle> {
        let matching: Vec<&PuzzleRecord> = self.filter(criteria).collect();
        if matching.is_empty() {
            return None;
        }
        let chosen = matching[rng.gen_range(0..matching.len())];
        let moves_packed = chosen.get_moves(self.move_pool);
        let moves = moves_packed.iter().map(|m| m.to_uci()).collect();
        let t_id = chosen.theme_id() as usize;
        let themes = if t_id < self.theme_dict.len() {
            self.theme_dict[t_id].to_theme_names().into_iter().map(String::from).collect()
        } else {
            Vec::new()
        };

        Some(Puzzle {
            id: chosen.id_string(),
            fen: chosen.fen_string(),
            moves,
            rating: chosen.rating(),
            themes,
        })
    }
}
