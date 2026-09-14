//! Fixed-size 40-byte Puzzle Record for zero-copy memory mapping.
//!
//! Layout (40 bytes, 8-byte aligned):
//! - board_occupied: u64 (8 bytes)
//! - board_pieces: [u8; 16] (16 bytes)
//! - id: u32 (4 bytes Base62)
//! - move_packed: u32 (4 bytes: bits 0..25 move_offset, bits 26..31 move_count)
//! - theme_rating_packed: u32 (4 bytes: bits 0..19 theme_id, bits 20..31 rating)
//! - fen_packed: u32 (4 bytes: bits 0..15 fen_state, bits 16..23 halfmove, bits 24..31 reserved)

use bytemuck::{Pod, Zeroable};
use crate::board::CompactBoard;
use crate::id::{decode_base62_id, encode_base62_id};
use crate::moves::PackedMove;
use crate::theme::ThemeMask;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct PuzzleRecord {
    /// 64-bit occupancy mask for piece placement
    pub board_occupied: u64,

    /// Up to 32 piece nibbles packed into 16 bytes
    pub board_pieces: [u8; 16],

    /// Encoded 5-character Base62 ID
    pub id_base62: u32,

    /// Bits 0..25: move_offset (up to 67M moves), Bits 26..31: move_count (up to 63 moves)
    pub move_packed: u32,

    /// Bits 0..19: theme_id (up to 1M unique themes), Bits 20..31: rating (up to 4095)
    pub theme_rating_packed: u32,

    /// Bits 0..15: fen_state, Bits 16..23: halfmove, Bits 24..31: reserved
    pub fen_packed: u32,
}

// Compile-time assertion that PuzzleRecord is exactly 40 bytes!
const _: () = assert!(std::mem::size_of::<PuzzleRecord>() == 40);
const _: () = assert!(std::mem::align_of::<PuzzleRecord>() == 8);

impl PuzzleRecord {
    #[inline(always)]
    pub fn new(
        id_str: &str,
        compact_board: &CompactBoard,
        rating: u16,
        theme_id: u32,
        move_offset: u32,
        move_count: u8,
        fen_state: u16,
        fen_halfmove: u8,
    ) -> Self {
        let id_base62 = encode_base62_id(id_str);
        let move_packed = (move_offset & 0x03FF_FFFF) | (((move_count as u32) & 0x3F) << 26);
        let theme_rating_packed = (theme_id & 0x000F_FFFF) | (((rating as u32) & 0x0FFF) << 20);
        let fen_packed = (fen_state as u32) | (((fen_halfmove as u32) & 0xFF) << 16);

        Self {
            board_occupied: compact_board.occupied,
            board_pieces: compact_board.pieces,
            id_base62,
            move_packed,
            theme_rating_packed,
            fen_packed,
        }
    }

    #[inline(always)]
    pub fn id_string(&self) -> String {
        decode_base62_id(self.id_base62)
    }

    #[inline(always)]
    pub fn rating(&self) -> u16 {
        ((self.theme_rating_packed >> 20) & 0x0FFF) as u16
    }

    #[inline(always)]
    pub fn theme_id(&self) -> u32 {
        self.theme_rating_packed & 0x000F_FFFF
    }

    #[inline(always)]
    pub fn move_offset(&self) -> usize {
        (self.move_packed & 0x03FF_FFFF) as usize
    }

    #[inline(always)]
    pub fn move_count(&self) -> usize {
        ((self.move_packed >> 26) & 0x3F) as usize
    }

    #[inline(always)]
    pub fn fen_state(&self) -> u16 {
        (self.fen_packed & 0xFFFF) as u16
    }

    #[inline(always)]
    pub fn fen_halfmove(&self) -> u8 {
        ((self.fen_packed >> 16) & 0xFF) as u8
    }

    #[inline(always)]
    pub fn compact_board(&self) -> CompactBoard {
        CompactBoard {
            occupied: self.board_occupied,
            pieces: self.board_pieces,
        }
    }

    /// Reconstructs the full FEN string.
    pub fn fen_string(&self) -> String {
        let mut fen = String::with_capacity(90);
        self.compact_board().write_fen_ranks(&mut fen);

        // Turn
        fen.push(' ');
        let state = self.fen_state();
        if (state & 1) == 0 {
            fen.push('w');
        } else {
            fen.push('b');
        }

        // Castling
        fen.push(' ');
        let castling = (state >> 1) & 0x0F;
        if castling == 0 {
            fen.push('-');
        } else {
            if (castling & (1 << 0)) != 0 { fen.push('K'); }
            if (castling & (1 << 1)) != 0 { fen.push('Q'); }
            if (castling & (1 << 2)) != 0 { fen.push('k'); }
            if (castling & (1 << 3)) != 0 { fen.push('q'); }
        }

        // En-passant
        fen.push(' ');
        let ep = (state >> 5) & 0x7F;
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

        // Halfmove & Fullmove
        fen.push(' ');
        fen.push_str(&self.fen_halfmove().to_string());
        fen.push_str(" 1");

        fen
    }

    /// Checks if this puzzle matches the required theme mask using the Theme Dictionary.
    #[inline(always)]
    pub fn has_themes_all(&self, required: ThemeMask, theme_dict: &[ThemeMask]) -> bool {
        let t_id = self.theme_id() as usize;
        if t_id < theme_dict.len() {
            theme_dict[t_id].contains_all(required)
        } else {
            false
        }
    }

    #[inline(always)]
    pub fn has_themes_any(&self, any: ThemeMask, theme_dict: &[ThemeMask]) -> bool {
        let t_id = self.theme_id() as usize;
        if t_id < theme_dict.len() {
            theme_dict[t_id].contains_any(any)
        } else {
            false
        }
    }

    /// Retrieves the solution moves using the given move pool slice.
    #[inline(always)]
    pub fn get_moves<'a>(&self, move_pool: &'a [PackedMove]) -> &'a [PackedMove] {
        let start = self.move_offset();
        let end = start + self.move_count();
        if end <= move_pool.len() {
            &move_pool[start..end]
        } else {
            &[]
        }
    }
}
