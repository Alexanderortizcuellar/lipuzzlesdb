//! Theme bitmask representation and registry for Lichess puzzle themes.
//!
//! Using a 128-bit bitmask (`u128`), we can represent any combination of 128 distinct themes
//! with zero-cost bitwise operations (AND / OR / NOT).

use bytemuck::{Pod, Zeroable};

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Pod, Zeroable)]
pub struct ThemeMask(pub u128);

pub const THEME_NAMES: &[&str] = &[
    "crushing",          // 1
    "hangingPiece",      // 2
    "long",              // 3
    "middlegame",        // 4
    "advantage",         // 5
    "endgame",           // 6
    "short",             // 7
    "rookEndgame",       // 8
    "fork",              // 9
    "pawnEndgame",       // 10
    "mate",              // 11
    "mateIn2",           // 12
    "operaMate",         // 13
    "master",            // 14
    "interference",      // 15
    "kingsideAttack",    // 16
    "veryLong",          // 17
    "zugzwang",          // 18
    "exposedKing",       // 19
    "skewer",            // 20
    "mateIn1",           // 21
    "oneMove",           // 22
    "opening",           // 23
    "pin",               // 24
    "quietMove",         // 25
    "backRankMate",      // 26
    "discoveredAttack",  // 27
    "sacrifice",         // 28
    "bishopEndgame",     // 29
    "bodenMate",         // 30
    "deflection",        // 31
    "morphysMate",       // 32
    "smotheredMate",     // 33
    "advancedPawn",      // 34
    "attraction",        // 35
    "promotion",         // 36
    "mateIn3",           // 37
    "masterVsMaster",    // 38
    "superGM",           // 39
    "queensideAttack",   // 40
    "knightEndgame",     // 41
    "cornerMate",        // 42
    "defensiveMove",     // 43
    "queenEndgame",      // 44
    "attackingF2F7",     // 45
    "queenRookEndgame",  // 46
    "clearance",         // 47
    "intermezzo",        // 48
    "equality",          // 49
    "enPassant",         // 50
    "pillsburysMate",    // 51
    "trappedPiece",      // 52
    "hookMate",          // 53
    "discoveredCheck",   // 54
    "xRayAttack",        // 55
    "capturingDefender", // 56
    "swallowstailMate",  // 57
    "doubleBishopMate",  // 58
    "doubleCheck",       // 59
    "arabianMate",       // 60
    "mateIn4",           // 61
    "epauletteMate",     // 62
    "vukovicMate",       // 63
    "dovetailMate",      // 64
    "triangleMate",      // 65
    "balestraMate",      // 66
    "collinearMove",     // 67
    "killBoxMate",       // 68
    "anastasiaMate",     // 69
    "blindSwineMate",    // 70
    "castling",          // 71
    "mateIn5",           // 72
    "underPromotion",    // 73
];

impl ThemeMask {
    pub const EMPTY: Self = ThemeMask(0);

    #[inline(always)]
    pub const fn new(raw: u128) -> Self {
        ThemeMask(raw)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    #[inline(always)]
    pub fn set_bit(&mut self, bit_index: u8) {
        if bit_index < 128 {
            self.0 |= 1u128 << bit_index;
        }
    }

    #[inline(always)]
    pub fn has_bit(&self, bit_index: u8) -> bool {
        if bit_index < 128 {
            (self.0 & (1u128 << bit_index)) != 0
        } else {
            false
        }
    }

    #[inline(always)]
    pub fn contains_all(&self, other: ThemeMask) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline(always)]
    pub fn contains_any(&self, other: ThemeMask) -> bool {
        (self.0 & other.0) != 0
    }

    /// Resolves theme name to bit index (0-indexed).
    pub fn theme_to_bit(name: &str) -> Option<u8> {
        let name_lower = name.trim().to_ascii_lowercase();
        THEME_NAMES
            .iter()
            .position(|&t| t.to_ascii_lowercase() == name_lower)
            .map(|pos| pos as u8)
    }

    /// Creates a ThemeMask from an iterator of theme names (e.g. ["fork", "endgame"]).
    pub fn from_names<'a, I>(names: I) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut mask = ThemeMask::EMPTY;
        for name in names {
            if let Some(bit) = Self::theme_to_bit(name) {
                mask.set_bit(bit);
            }
        }
        mask
    }

    /// Creates a ThemeMask from 1-based SQLite theme IDs (e.g. [1, 2, 3, 4]).
    pub fn from_1based_ids<I>(ids: I) -> Self
    where
        I: IntoIterator<Item = u16>,
    {
        let mut mask = ThemeMask::EMPTY;
        for id in ids {
            if id >= 1 && id <= 128 {
                mask.set_bit((id - 1) as u8);
            }
        }
        mask
    }

    /// Parses a space-separated SQLite theme ID string (e.g. "1 2 3 4").
    pub fn from_sqlite_ids_str(s: &str) -> Self {
        let mut mask = ThemeMask::EMPTY;
        for part in s.split_whitespace() {
            if let Ok(id) = part.parse::<u16>() {
                if id >= 1 && id <= 128 {
                    mask.set_bit((id - 1) as u8);
                }
            }
        }
        mask
    }

    /// Parses a space-separated Lichess CSV themes string (e.g. "advantage fork short").
    pub fn from_csv_themes_str(s: &str) -> Self {
        Self::from_names(s.split_whitespace())
    }

    /// Returns the list of canonical theme names present in this mask.
    pub fn to_theme_names(&self) -> Vec<&'static str> {
        let mut list = Vec::new();
        for (i, &name) in THEME_NAMES.iter().enumerate() {
            if self.has_bit(i as u8) {
                list.push(name);
            }
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_mask() {
        let mask = ThemeMask::from_names(["fork", "endgame", "mateIn2"]);
        assert!(mask.has_bit(ThemeMask::theme_to_bit("fork").unwrap()));
        assert!(mask.has_bit(ThemeMask::theme_to_bit("endgame").unwrap()));
        assert!(mask.has_bit(ThemeMask::theme_to_bit("mateIn2").unwrap()));
        assert!(!mask.has_bit(ThemeMask::theme_to_bit("quietMove").unwrap()));

        let query = ThemeMask::from_names(["fork"]);
        assert!(mask.contains_all(query));
        assert!(mask.contains_any(query));

        let query_not = ThemeMask::from_names(["quietMove"]);
        assert!(!mask.contains_all(query_not));
        assert!(!mask.contains_any(query_not));
    }

    #[test]
    fn test_sqlite_str_parse() {
        // "1 2 3 4" => crushing, hangingPiece, long, middlegame
        let mask = ThemeMask::from_sqlite_ids_str("1 2 3 4");
        let names = mask.to_theme_names();
        assert_eq!(names, vec!["crushing", "hangingPiece", "long", "middlegame"]);
    }
}
