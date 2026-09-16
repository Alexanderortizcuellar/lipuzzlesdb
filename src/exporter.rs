//! PGN and CSV Exporter for Chess Puzzles
//!
//! Converts internal `Puzzle` structs into standard PGN (Portable Game Notation)
//! with proper tags (Event, Site, Date, White, Black, Result, FEN, SetUp, PuzzleId, Rating, Themes)
//! and algebraic move formatting, or into CSV format.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use crate::db::Puzzle;

pub struct Exporter;

impl Exporter {
    /// Formats a single puzzle into standard PGN format.
    ///
    /// The PGN includes full FEN setup, puzzle metadata tags, and the SAN/UCI move sequence.
    pub fn puzzle_to_pgn(puzzle: &Puzzle) -> String {
        let mut pgn = String::with_capacity(512);

        // Header tags
        pgn.push_str(&format!("[Event \"Lichess Puzzle {}\"]\n", puzzle.id));
        pgn.push_str("[Site \"https://lichess.org/training\"]\n");
        pgn.push_str("[Date \"????.??.??\"]\n");
        pgn.push_str("[White \"Puzzle\"]\n");
        pgn.push_str("[Black \"Puzzle\"]\n");
        pgn.push_str("[Result \"*\"]\n");
        pgn.push_str("[SetUp \"1\"]\n");
        pgn.push_str(&format!("[FEN \"{}\"]\n", puzzle.fen));
        pgn.push_str(&format!("[PuzzleId \"{}\"]\n", puzzle.id));
        pgn.push_str(&format!("[Rating \"{}\"]\n", puzzle.rating));
        if !puzzle.themes.is_empty() {
            pgn.push_str(&format!("[Themes \"{}\"]\n", puzzle.themes.join(" ")));
        }
        pgn.push('\n');

        // Move text with ply/move numbering
        let fen_parts: Vec<&str> = puzzle.fen.split_whitespace().collect();
        let is_black_to_move = fen_parts.get(1).map(|&c| c == "b").unwrap_or(false);
        let fullmove_number = fen_parts.get(5).and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);

        let mut current_move = fullmove_number;
        let mut is_black = is_black_to_move;

        for (i, mv) in puzzle.moves.iter().enumerate() {
            if i == 0 {
                if is_black {
                    pgn.push_str(&format!("{}... {} ", current_move, mv));
                    current_move += 1;
                    is_black = false;
                } else {
                    pgn.push_str(&format!("{}. {} ", current_move, mv));
                    is_black = true;
                }
            } else if !is_black {
                pgn.push_str(&format!("{}. {} ", current_move, mv));
                is_black = true;
            } else {
                pgn.push_str(&format!("{} ", mv));
                current_move += 1;
                is_black = false;
            }
        }

        pgn.push_str("*\n\n");
        pgn
    }

    /// Exports a slice or iterator of puzzles to a PGN file.
    pub fn export_to_pgn_file<'a, I>(puzzles: I, path: impl AsRef<Path>) -> io::Result<usize>
    where
        I: IntoIterator<Item = &'a Puzzle>,
    {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        let mut count = 0;

        for p in puzzles {
            let pgn_text = Self::puzzle_to_pgn(p);
            writer.write_all(pgn_text.as_bytes())?;
            count += 1;
        }

        writer.flush()?;
        Ok(count)
    }

    /// Formats a single puzzle into a standard CSV line:
    /// `PuzzleId,FEN,Moves,Rating,Themes`
    pub fn puzzle_to_csv_line(puzzle: &Puzzle) -> String {
        let escaped_fen = if puzzle.fen.contains(',') {
            format!("\"{}\"", puzzle.fen)
        } else {
            puzzle.fen.clone()
        };

        let moves_str = puzzle.moves.join(" ");
        let themes_str = puzzle.themes.join(" ");

        format!(
            "{},{},{},{},\"{}\"\n",
            puzzle.id, escaped_fen, moves_str, puzzle.rating, themes_str
        )
    }

    /// Exports a slice or iterator of puzzles to a CSV file.
    pub fn export_to_csv_file<'a, I>(puzzles: I, path: impl AsRef<Path>) -> io::Result<usize>
    where
        I: IntoIterator<Item = &'a Puzzle>,
    {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        let mut count = 0;

        // CSV Header
        writer.write_all(b"PuzzleId,FEN,Moves,Rating,Themes\n")?;

        for p in puzzles {
            let line = Self::puzzle_to_csv_line(p);
            writer.write_all(line.as_bytes())?;
            count += 1;
        }

        writer.flush()?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_puzzle_to_pgn_white_to_move() {
        let p = Puzzle {
            id: "00008".to_string(),
            fen: "r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K w - - 0 24".to_string(),
            moves: vec!["f6e6".to_string(), "d5e6".to_string(), "c1b2".to_string()],
            rating: 1850,
            themes: vec!["crushing".to_string(), "hangingPiece".to_string()],
        };

        let pgn = Exporter::puzzle_to_pgn(&p);
        assert!(pgn.contains("[PuzzleId \"00008\"]"));
        assert!(pgn.contains("[Rating \"1850\"]"));
        assert!(pgn.contains("[Themes \"crushing hangingPiece\"]"));
        assert!(pgn.contains("24. f6e6 d5e6 25. c1b2 *"));
    }

    #[test]
    fn test_puzzle_to_pgn_black_to_move() {
        let p = Puzzle {
            id: "0000d".to_string(),
            fen: "5rk1/1p3ppp/pq3b2/8/8/1P1Q1N2/P4PPP/3R2K1 b - - 2 27".to_string(),
            moves: vec!["f8d8".to_string(), "d3e2".to_string()],
            rating: 1500,
            themes: vec!["advantage".to_string(), "short".to_string()],
        };

        let pgn = Exporter::puzzle_to_pgn(&p);
        assert!(pgn.contains("27... f8d8 28. d3e2 *"));
    }
}
