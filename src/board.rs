//! Bitboard-compressed board representation (24 bytes: 8 bytes occupancy + 16 bytes pieces).
//!
//! Fits up to 32 pieces (physical maximum in standard chess).

use bytemuck::{Pod, Zeroable};
use crate::fen::nibble_to_piece;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct CompactBoard {
    /// 64-bit occupancy mask: bit i = 1 if square i (0..63) has a piece.
    pub occupied: u64,
    /// Up to 32 piece nibbles packed into 16 bytes (2 pieces per byte).
    pub pieces: [u8; 16],
}

const _: () = assert!(std::mem::size_of::<CompactBoard>() == 24);

impl CompactBoard {
    pub const EMPTY: Self = CompactBoard {
        occupied: 0,
        pieces: [0; 16],
    };

    /// Encodes a 32-byte (64-square nibble) board into a CompactBoard.
    pub fn from_board_nibbles(board: &[u8; 32]) -> Self {
        let mut occupied = 0u64;
        let mut piece_nibbles = [0u8; 32];
        let mut piece_count = 0;

        for sq in 0..64 {
            let byte = board[sq / 2];
            let nibble = if sq % 2 == 0 { byte >> 4 } else { byte & 0x0F };
            if nibble != 0 {
                occupied |= 1u64 << sq;
                if piece_count < 32 {
                    piece_nibbles[piece_count] = nibble;
                    piece_count += 1;
                }
            }
        }

        let mut pieces = [0u8; 16];
        for i in 0..16 {
            pieces[i] = (piece_nibbles[i * 2] << 4) | (piece_nibbles[i * 2 + 1] & 0x0F);
        }

        CompactBoard { occupied, pieces }
    }

    /// Decodes back to a 32-byte (64-square nibble) board.
    #[inline(always)]
    pub fn to_board_nibbles(&self) -> [u8; 32] {
        let mut board = [0u8; 32];
        let mut piece_idx = 0;
        let mut occ = self.occupied;

        while occ != 0 {
            let sq = occ.trailing_zeros() as usize;
            occ &= occ - 1; // Clear lowest set bit

            if piece_idx < 32 {
                let byte = self.pieces[piece_idx / 2];
                let nibble = if piece_idx % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                if sq % 2 == 0 {
                    board[sq / 2] |= nibble << 4;
                } else {
                    board[sq / 2] |= nibble & 0x0F;
                }
                piece_idx += 1;
            }
        }

        board
    }

    /// Reconstructs the board rank strings for FEN.
    pub fn write_fen_ranks(&self, result: &mut String) {
        let board_nibbles = self.to_board_nibbles();
        for rank in 0..8 {
            let mut empty_run = 0;
            for file in 0..8 {
                let sq = rank * 8 + file;
                let byte_val = board_nibbles[sq / 2];
                let nibble = if sq % 2 == 0 { byte_val >> 4 } else { byte_val & 0x0F };

                if nibble == 0 {
                    empty_run += 1;
                } else {
                    if empty_run > 0 {
                        result.push_str(&empty_run.to_string());
                        empty_run = 0;
                    }
                    if let Some(p) = nibble_to_piece(nibble) {
                        result.push(p);
                    }
                }
            }
            if empty_run > 0 {
                result.push_str(&empty_run.to_string());
            }
            if rank < 7 {
                result.push('/');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fen::PackedFen;

    #[test]
    fn test_compact_board_roundtrip() {
        let fens = [
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K b - - 0 24",
            "5rk1/1p3ppp/pq3b2/8/8/1P1Q1N2/P4PPP/3R2K1 w - - 2 27",
            "8/4R3/1p2P3/p4r2/P6p/1P3Pk1/4K3/8 w - - 1 64",
            "8/8/8/8/8/8/8/8 w - - 0 1",
            "4k3/8/8/8/8/8/8/4K3 w - - 0 1",
        ];

        for fen in fens {
            let packed = PackedFen::encode(fen).unwrap();
            let compact = CompactBoard::from_board_nibbles(&packed.board);
            let reconstructed_nibbles = compact.to_board_nibbles();
            assert_eq!(reconstructed_nibbles, packed.board);

            let mut fen_ranks = String::new();
            compact.write_fen_ranks(&mut fen_ranks);
            let original_ranks = fen.split_whitespace().next().unwrap();
            assert_eq!(fen_ranks, original_ranks);
        }
    }
}
