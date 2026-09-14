//! Compact 16-bit UCI move encoder and decoder.
//!
use bytemuck::{Pod, Zeroable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Pod, Zeroable)]
#[repr(transparent)]
pub struct PackedMove(pub u16);

impl PackedMove {
    #[inline(always)]
    pub const fn new(from_sq: u8, to_sq: u8, promo: u8) -> Self {
        let val = ((from_sq as u16) & 0x3F)
            | (((to_sq as u16) & 0x3F) << 6)
            | (((promo as u16) & 0x07) << 12);
        PackedMove(val)
    }

    #[inline(always)]
    pub const fn from_sq(&self) -> u8 {
        (self.0 & 0x3F) as u8
    }

    #[inline(always)]
    pub const fn to_sq(&self) -> u8 {
        ((self.0 >> 6) & 0x3F) as u8
    }

    #[inline(always)]
    pub const fn promo_raw(&self) -> u8 {
        ((self.0 >> 12) & 0x07) as u8
    }

    #[inline(always)]
    pub const fn promo_char(&self) -> Option<char> {
        match self.promo_raw() {
            1 => Some('q'),
            2 => Some('r'),
            3 => Some('b'),
            4 => Some('n'),
            5 => Some('k'),
            _ => None,
        }
    }

    /// Parses a UCI move string (e.g. "e2e4", "e7e8q") into a PackedMove.
    pub fn from_uci(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() < 4 || bytes.len() > 5 {
            return None;
        }

        let f_file = bytes[0].checked_sub(b'a')?;
        let f_rank = bytes[1].checked_sub(b'1')?;
        let t_file = bytes[2].checked_sub(b'a')?;
        let t_rank = bytes[3].checked_sub(b'1')?;

        if f_file > 7 || f_rank > 7 || t_file > 7 || t_rank > 7 {
            return None;
        }

        let from_sq = f_rank * 8 + f_file;
        let to_sq = t_rank * 8 + t_file;

        let promo = if bytes.len() == 5 {
            match bytes[4] {
                b'q' | b'Q' => 1,
                b'r' | b'R' => 2,
                b'b' | b'B' => 3,
                b'n' | b'N' => 4,
                b'k' | b'K' => 5,
                _ => return None,
            }
        } else {
            0
        };

        Some(Self::new(from_sq, to_sq, promo))
    }

    /// Converts back to a standard UCI string (e.g. "e2e4", "e7e8q").
    pub fn to_uci(&self) -> String {
        let f_sq = self.from_sq();
        let t_sq = self.to_sq();

        let f_file = (b'a' + (f_sq % 8)) as char;
        let f_rank = (b'1' + (f_sq / 8)) as char;
        let t_file = (b'a' + (t_sq % 8)) as char;
        let t_rank = (b'1' + (t_sq / 8)) as char;

        if let Some(p) = self.promo_char() {
            format!("{}{}{}{}{}", f_file, f_rank, t_file, t_rank, p)
        } else {
            format!("{}{}{}{}", f_file, f_rank, t_file, t_rank)
        }
    }
}

/// Encodes a space-separated UCI move sequence into a Vec<PackedMove>.
pub fn encode_moves(moves_str: &str) -> Vec<PackedMove> {
    moves_str
        .split_whitespace()
        .filter_map(PackedMove::from_uci)
        .collect()
}

/// Decodes a slice of PackedMove into a space-separated UCI move string.
pub fn decode_moves(moves: &[PackedMove]) -> String {
    moves
        .iter()
        .map(|m| m.to_uci())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_roundtrip() {
        let tests = [
            "e2e4",
            "g1f3",
            "e7e8q",
            "a7a8r",
            "c7c8b",
            "h7h8n",
            "f2f1q",
        ];

        for m_str in tests {
            let pm = PackedMove::from_uci(m_str).expect("failed to parse move");
            assert_eq!(pm.to_uci(), m_str);
        }
    }

    #[test]
    fn test_move_sequence_roundtrip() {
        let line = "f2g3 e6e7 b2b1 b3c1 b1c1 h6c1";
        let encoded = encode_moves(line);
        assert_eq!(encoded.len(), 6);
        let decoded = decode_moves(&encoded);
        assert_eq!(decoded, line);
    }
}
