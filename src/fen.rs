//! Packed FEN encoder and decoder for chess positions.
//!
//! Compact representation:
//! - 64 squares packed into 32 bytes (4 bits / nibble per square)
//! - Turn, Castling rights, and En-passant square packed into a u16
//! - Halfmove clock in u8, Fullmove in u16

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenError {
    InvalidRankCount,
    InvalidSquareCount,
    InvalidPiece(char),
    InvalidTurn,
    InvalidCastling,
    InvalidEnPassant,
    InvalidHalfmove,
    InvalidFullmove,
    MalformedFen,
}

impl std::fmt::Display for FenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for FenError {}

/// 4-bit piece encoding:
/// 0: Empty
/// 1: White Pawn (P)
/// 2: White Knight (N)
/// 3: White Bishop (B)
/// 4: White Rook (R)
/// 5: White Queen (Q)
/// 6: White King (K)
/// 7: Black Pawn (p)
/// 8: Black Knight (n)
/// 9: Black Bishop (b)
/// 10: Black Rook (r)
/// 11: Black Queen (q)
/// 12: Black King (k)
#[inline(always)]
pub fn piece_to_nibble(c: char) -> Result<u8, FenError> {
    match c {
        'P' => Ok(1),
        'N' => Ok(2),
        'B' => Ok(3),
        'R' => Ok(4),
        'Q' => Ok(5),
        'K' => Ok(6),
        'p' => Ok(7),
        'n' => Ok(8),
        'b' => Ok(9),
        'r' => Ok(10),
        'q' => Ok(11),
        'k' => Ok(12),
        _ => Err(FenError::InvalidPiece(c)),
    }
}

#[inline(always)]
pub fn nibble_to_piece(n: u8) -> Option<char> {
    match n {
        1 => Some('P'),
        2 => Some('N'),
        3 => Some('B'),
        4 => Some('R'),
        5 => Some('Q'),
        6 => Some('K'),
        7 => Some('p'),
        8 => Some('n'),
        9 => Some('b'),
        10 => Some('r'),
        11 => Some('q'),
        12 => Some('k'),
        _ => None,
    }
}

#[repr(C, align(4))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedFen {
    /// 64 squares stored as 32 bytes (2 squares per byte).
    /// Byte 0: Sq 0 (high nibble), Sq 1 (low nibble)
    pub board: [u8; 32],
    /// Bits 0: Turn (0 = White, 1 = Black)
    /// Bits 1..4: Castling rights (1: K, 2: Q, 4: k, 8: q)
    /// Bits 5..11: En-passant square (0 = None, 1..64 = square index + 1)
    pub state: u16,
    /// Halfmove clock (50-move rule)
    pub halfmove: u8,
    /// Fullmove number
    pub fullmove: u16,
}

impl PackedFen {
    /// Encodes a standard FEN string into a PackedFen.
    pub fn encode(fen: &str) -> Result<Self, FenError> {
        let mut parts = fen.split_whitespace();
        let piece_placement = parts.next().ok_or(FenError::MalformedFen)?;
        let turn_str = parts.next().unwrap_or("w");
        let castling_str = parts.next().unwrap_or("-");
        let ep_str = parts.next().unwrap_or("-");
        let halfmove_str = parts.next().unwrap_or("0");
        let fullmove_str = parts.next().unwrap_or("1");

        // Parse board
        let mut squares = [0u8; 64];
        let mut sq_idx = 0;
        let ranks: Vec<&str> = piece_placement.split('/').collect();
        if ranks.len() != 8 {
            return Err(FenError::InvalidRankCount);
        }

        for rank_str in ranks {
            let mut file_count = 0;
            for c in rank_str.chars() {
                if let Some(digit) = c.to_digit(10) {
                    let empty_count = digit as usize;
                    file_count += empty_count;
                    sq_idx += empty_count;
                } else {
                    let nibble = piece_to_nibble(c)?;
                    if sq_idx >= 64 {
                        return Err(FenError::InvalidSquareCount);
                    }
                    squares[sq_idx] = nibble;
                    sq_idx += 1;
                    file_count += 1;
                }
            }
            if file_count != 8 {
                return Err(FenError::InvalidSquareCount);
            }
        }

        if sq_idx != 64 {
            return Err(FenError::InvalidSquareCount);
        }

        let mut board = [0u8; 32];
        for i in 0..32 {
            board[i] = (squares[i * 2] << 4) | (squares[i * 2 + 1] & 0x0F);
        }

        // Parse turn
        let turn_bit: u16 = match turn_str {
            "w" => 0,
            "b" => 1,
            _ => return Err(FenError::InvalidTurn),
        };

        // Parse castling
        let mut castling_bits: u16 = 0;
        if castling_str != "-" {
            for c in castling_str.chars() {
                match c {
                    'K' => castling_bits |= 1 << 0,
                    'Q' => castling_bits |= 1 << 1,
                    'k' => castling_bits |= 1 << 2,
                    'q' => castling_bits |= 1 << 3,
                    _ => return Err(FenError::InvalidCastling),
                }
            }
        }

        // Parse En-passant
        let ep_bits: u16 = if ep_str == "-" {
            0
        } else if ep_str.len() == 2 {
            let file_char = ep_str.chars().next().unwrap();
            let rank_char = ep_str.chars().nth(1).unwrap();
            if !('a'..='h').contains(&file_char) || !('1'..='8').contains(&rank_char) {
                return Err(FenError::InvalidEnPassant);
            }
            let file = (file_char as u8) - b'a';
            let rank = 7 - ((rank_char as u8) - b'1');
            let sq = (rank * 8 + file) as u16;
            sq + 1
        } else {
            return Err(FenError::InvalidEnPassant);
        };

        let state = turn_bit | (castling_bits << 1) | (ep_bits << 5);

        let halfmove = halfmove_str.parse::<u8>().unwrap_or(0);
        let fullmove = fullmove_str.parse::<u16>().unwrap_or(1);

        Ok(PackedFen {
            board,
            state,
            halfmove,
            fullmove,
        })
    }

    /// Decodes back to a standard FEN string.
    pub fn to_fen(&self) -> String {
        let mut result = String::with_capacity(90);

        // Decode board
        for rank in 0..8 {
            let mut empty_run = 0;
            for file in 0..8 {
                let sq = rank * 8 + file;
                let byte_val = self.board[sq / 2];
                let nibble = if sq % 2 == 0 {
                    byte_val >> 4
                } else {
                    byte_val & 0x0F
                };

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

        // Decode turn
        result.push(' ');
        if (self.state & 1) == 0 {
            result.push('w');
        } else {
            result.push('b');
        }

        // Decode castling
        result.push(' ');
        let castling = (self.state >> 1) & 0x0F;
        if castling == 0 {
            result.push('-');
        } else {
            if (castling & (1 << 0)) != 0 { result.push('K'); }
            if (castling & (1 << 1)) != 0 { result.push('Q'); }
            if (castling & (1 << 2)) != 0 { result.push('k'); }
            if (castling & (1 << 3)) != 0 { result.push('q'); }
        }

        // Decode En-passant
        result.push(' ');
        let ep = (self.state >> 5) & 0x7F;
        if ep == 0 || ep > 64 {
            result.push('-');
        } else {
            let sq = (ep - 1) as u8;
            let rank = sq / 8;
            let file = sq % 8;
            let file_char = (b'a' + file) as char;
            let rank_char = (b'1' + (7 - rank)) as char;
            result.push(file_char);
            result.push(rank_char);
        }

        // Halfmove & Fullmove
        result.push(' ');
        result.push_str(&self.halfmove.to_string());
        result.push(' ');
        result.push_str(&self.fullmove.to_string());

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starting_fen_roundtrip() {
        let fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
        let packed = PackedFen::encode(fen).unwrap();
        assert_eq!(packed.to_fen(), fen);
    }

    #[test]
    fn test_puzzle_fens_roundtrip() {
        let fens = [
            "r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K b - - 0 24",
            "5rk1/1p3ppp/pq3b2/8/8/1P1Q1N2/P4PPP/3R2K1 w - - 2 27",
            "8/4R3/1p2P3/p4r2/P6p/1P3Pk1/4K3/8 w - - 1 64",
            "r2qr1k1/b1p2ppp/pp4n1/P1P1p3/4P1n1/B2P2Pb/3NBP1P/RN1QR1K1 b - - 1 16",
            "r1b1k2r/pppp1ppp/8/4n3/1bP2B2/8/PP1NPPPP/R2QKB1R w KQkq e6 0 8",
        ];

        for fen in fens {
            let packed = PackedFen::encode(fen).expect("failed to encode fen");
            let decoded = packed.to_fen();
            assert_eq!(decoded, fen);
        }
    }
}
