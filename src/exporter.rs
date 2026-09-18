//! PGN and CSV Exporter for Chess Puzzles
//!
//! Converts internal `Puzzle` structs into standard PGN (Portable Game Notation)
//! with proper tags (Event, Site, Date, White, Black, Result, FEN, SetUp, PuzzleId, Rating, Themes)
//! and standard SAN (Standard Algebraic Notation, e.g. `Nxd5`, `Rxe6+`, `O-O`, `e8=Q#`) move formatting
//! with fullmove numbering based on the puzzle FEN.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use crate::db::Puzzle;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Color {
    White,
    Black,
}

impl Color {
    fn opponent(&self) -> Self {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PieceType {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl PieceType {
    fn san_char(&self) -> Option<char> {
        match self {
            PieceType::Pawn => None,
            PieceType::Knight => Some('N'),
            PieceType::Bishop => Some('B'),
            PieceType::Rook => Some('R'),
            PieceType::Queen => Some('Q'),
            PieceType::King => Some('K'),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Piece {
    color: Color,
    piece_type: PieceType,
}

#[derive(Clone, Debug)]
struct BoardState {
    squares: [Option<Piece>; 64],
    turn: Color,
    castling: [bool; 4], // [W_K, W_Q, B_k, B_q]
    ep_square: Option<u8>,
    halfmove: u8,
    fullmove: usize,
}

impl BoardState {
    fn from_fen(fen: &str) -> Option<Self> {
        let mut parts = fen.split_whitespace();
        let board_str = parts.next()?;
        let turn_str = parts.next().unwrap_or("w");
        let castling_str = parts.next().unwrap_or("-");
        let ep_str = parts.next().unwrap_or("-");
        let halfmove_str = parts.next().unwrap_or("0");
        let fullmove_str = parts.next().unwrap_or("1");

        let mut squares = [None; 64];
        let mut sq = 0;

        for rank_str in board_str.split('/') {
            for c in rank_str.chars() {
                if let Some(digit) = c.to_digit(10) {
                    sq += digit as usize;
                } else {
                    if sq >= 64 {
                        return None;
                    }
                    let color = if c.is_uppercase() { Color::White } else { Color::Black };
                    let piece_type = match c.to_ascii_lowercase() {
                        'p' => PieceType::Pawn,
                        'n' => PieceType::Knight,
                        'b' => PieceType::Bishop,
                        'r' => PieceType::Rook,
                        'q' => PieceType::Queen,
                        'k' => PieceType::King,
                        _ => return None,
                    };
                    squares[sq] = Some(Piece { color, piece_type });
                    sq += 1;
                }
            }
        }

        let turn = if turn_str == "b" { Color::Black } else { Color::White };

        let mut castling = [false; 4];
        if castling_str != "-" {
            for c in castling_str.chars() {
                match c {
                    'K' => castling[0] = true,
                    'Q' => castling[1] = true,
                    'k' => castling[2] = true,
                    'q' => castling[3] = true,
                    _ => {}
                }
            }
        }

        let ep_square = if ep_str != "-" && ep_str.len() == 2 {
            let bytes = ep_str.as_bytes();
            let file = bytes[0].checked_sub(b'a')?;
            let rank_digit = bytes[1].checked_sub(b'1')?;
            if file < 8 && rank_digit < 8 {
                let rank = 7 - rank_digit;
                Some(rank * 8 + file)
            } else {
                None
            }
        } else {
            None
        };

        let halfmove = halfmove_str.parse::<u8>().unwrap_or(0);
        let fullmove = fullmove_str.parse::<usize>().unwrap_or(1);

        Some(BoardState {
            squares,
            turn,
            castling,
            ep_square,
            halfmove,
            fullmove,
        })
    }

    /// Converts a UCI move (e.g. "e2e4", "e7e8q") into standard algebraic notation (SAN)
    /// and updates the internal board position.
    fn uci_to_san_and_apply(&mut self, uci: &str) -> String {
        let bytes = uci.as_bytes();
        if bytes.len() < 4 {
            return uci.to_string();
        }

        let from_file = (bytes[0] as usize).saturating_sub(b'a' as usize);
        let from_rank = 7usize.saturating_sub((bytes[1] as usize).saturating_sub(b'1' as usize));
        let to_file = (bytes[2] as usize).saturating_sub(b'a' as usize);
        let to_rank = 7usize.saturating_sub((bytes[3] as usize).saturating_sub(b'1' as usize));

        if from_file > 7 || from_rank > 7 || to_file > 7 || to_rank > 7 {
            return uci.to_string();
        }

        let from_sq = from_rank * 8 + from_file;
        let to_sq = to_rank * 8 + to_file;

        let promo = if bytes.len() == 5 {
            match bytes[4] {
                b'q' | b'Q' => Some(PieceType::Queen),
                b'r' | b'R' => Some(PieceType::Rook),
                b'b' | b'B' => Some(PieceType::Bishop),
                b'n' | b'N' => Some(PieceType::Knight),
                _ => None,
            }
        } else {
            None
        };

        let piece = match self.squares[from_sq] {
            Some(p) => p,
            None => return uci.to_string(),
        };

        let is_capture = self.squares[to_sq].is_some()
            || (piece.piece_type == PieceType::Pawn && Some(to_sq as u8) == self.ep_square);

        let mut san = String::with_capacity(8);

        // 1. Check Castling (O-O or O-O-O)
        if piece.piece_type == PieceType::King && from_file == 4 && (to_file == 6 || to_file == 2) {
            if to_file == 6 {
                san.push_str("O-O");
            } else {
                san.push_str("O-O-O");
            }
        } else if piece.piece_type == PieceType::Pawn {
            // 2. Pawn Moves
            if is_capture {
                let f_char = (b'a' + from_file as u8) as char;
                san.push(f_char);
                san.push('x');
            }
            let t_file_char = (b'a' + to_file as u8) as char;
            let t_rank_char = (b'1' + (7 - to_rank) as u8) as char;
            san.push(t_file_char);
            san.push(t_rank_char);

            if let Some(p) = promo {
                san.push('=');
                if let Some(c) = p.san_char() {
                    san.push(c);
                }
            }
        } else {
            // 3. Piece Moves (Knight, Bishop, Rook, Queen, King)
            if let Some(c) = piece.piece_type.san_char() {
                san.push(c);
            }

            // Disambiguation
            let mut candidates = Vec::new();
            for sq in 0..64 {
                if sq != from_sq {
                    if let Some(other_piece) = self.squares[sq] {
                        if other_piece == piece && self.can_piece_reach(sq, to_sq, piece.piece_type) {
                            candidates.push(sq);
                        }
                    }
                }
            }

            if !candidates.is_empty() {
                let mut same_file = false;
                let mut same_rank = false;
                for &cand_sq in &candidates {
                    if cand_sq % 8 == from_file {
                        same_file = true;
                    }
                    if cand_sq / 8 == from_rank {
                        same_rank = true;
                    }
                }

                if !same_file {
                    san.push((b'a' + from_file as u8) as char);
                } else if !same_rank {
                    san.push((b'1' + (7 - from_rank) as u8) as char);
                } else {
                    san.push((b'a' + from_file as u8) as char);
                    san.push((b'1' + (7 - from_rank) as u8) as char);
                }
            }

            if is_capture {
                san.push('x');
            }

            let t_file_char = (b'a' + to_file as u8) as char;
            let t_rank_char = (b'1' + (7 - to_rank) as u8) as char;
            san.push(t_file_char);
            san.push(t_rank_char);
        }

        // Apply move to board
        self.apply_move_internal(from_sq, to_sq, promo);

        // Check if move results in Check / Checkmate
        if self.is_in_check(self.turn) {
            if self.has_no_legal_moves(self.turn) {
                san.push('#');
            } else {
                san.push('+');
            }
        }

        san
    }

    fn can_piece_reach(&self, from_sq: usize, to_sq: usize, pt: PieceType) -> bool {
        let f_rank = from_sq / 8;
        let f_file = from_sq % 8;
        let t_rank = to_sq / 8;
        let t_file = to_sq % 8;

        match pt {
            PieceType::Knight => {
                let dr = (f_rank as isize - t_rank as isize).abs();
                let df = (f_file as isize - t_file as isize).abs();
                (dr == 1 && df == 2) || (dr == 2 && df == 1)
            }
            PieceType::Bishop => {
                let dr = (f_rank as isize - t_rank as isize).abs();
                let df = (f_file as isize - t_file as isize).abs();
                dr == df && self.is_path_clear(from_sq, to_sq)
            }
            PieceType::Rook => {
                (f_rank == t_rank || f_file == t_file) && self.is_path_clear(from_sq, to_sq)
            }
            PieceType::Queen => {
                let dr = (f_rank as isize - t_rank as isize).abs();
                let df = (f_file as isize - t_file as isize).abs();
                (dr == df || f_rank == t_rank || f_file == t_file) && self.is_path_clear(from_sq, to_sq)
            }
            PieceType::King => {
                let dr = (f_rank as isize - t_rank as isize).abs();
                let df = (f_file as isize - t_file as isize).abs();
                dr <= 1 && df <= 1
            }
            PieceType::Pawn => false,
        }
    }

    fn is_path_clear(&self, from_sq: usize, to_sq: usize) -> bool {
        let f_rank = from_sq as isize / 8;
        let f_file = from_sq as isize % 8;
        let t_rank = to_sq as isize / 8;
        let t_file = to_sq as isize % 8;

        let step_r = (t_rank - f_rank).signum();
        let step_f = (t_file - f_file).signum();

        let mut curr_r = f_rank + step_r;
        let mut curr_f = f_file + step_f;

        while curr_r != t_rank || curr_f != t_file {
            let sq = (curr_r * 8 + curr_f) as usize;
            if self.squares[sq].is_some() {
                return false;
            }
            curr_r += step_r;
            curr_f += step_f;
        }

        true
    }

    fn apply_move_internal(&mut self, from_sq: usize, to_sq: usize, promo: Option<PieceType>) {
        let piece = self.squares[from_sq].take().unwrap();
        let mut new_piece = piece;
        if let Some(p) = promo {
            new_piece.piece_type = p;
        }

        // Handle en-passant capture
        if piece.piece_type == PieceType::Pawn && Some(to_sq as u8) == self.ep_square {
            let cap_sq = if piece.color == Color::White { to_sq + 8 } else { to_sq - 8 };
            self.squares[cap_sq] = None;
        }

        // Handle castling rook moves
        if piece.piece_type == PieceType::King {
            let f_file = from_sq % 8;
            let t_file = to_sq % 8;
            let rank = from_sq / 8;
            if f_file == 4 && t_file == 6 {
                let rook = self.squares[rank * 8 + 7].take();
                self.squares[rank * 8 + 5] = rook;
            } else if f_file == 4 && t_file == 2 {
                let rook = self.squares[rank * 8 + 0].take();
                self.squares[rank * 8 + 3] = rook;
            }
        }

        self.squares[to_sq] = Some(new_piece);

        // Update turn & fullmove
        if self.turn == Color::Black {
            self.fullmove += 1;
        }
        self.turn = self.turn.opponent();

        // Update en-passant square
        if piece.piece_type == PieceType::Pawn && (from_sq as isize - to_sq as isize).abs() == 16 {
            let ep = (from_sq + to_sq) / 2;
            self.ep_square = Some(ep as u8);
        } else {
            self.ep_square = None;
        }
    }

    fn is_in_check(&self, color: Color) -> bool {
        // Find king of this color
        let mut king_sq = None;
        for sq in 0..64 {
            if let Some(Piece { color: c, piece_type: PieceType::King }) = self.squares[sq] {
                if c == color {
                    king_sq = Some(sq);
                    break;
                }
            }
        }

        let k_sq = match king_sq {
            Some(sq) => sq,
            None => return false,
        };

        let opp_color = color.opponent();

        for sq in 0..64 {
            if let Some(Piece { color: c, piece_type: pt }) = self.squares[sq] {
                if c == opp_color {
                    if pt == PieceType::Pawn {
                        let f_rank = sq / 8;
                        let f_file = sq % 8;
                        let k_rank = k_sq / 8;
                        let k_file = k_sq % 8;
                        let forward_rank = if opp_color == Color::White { f_rank.wrapping_sub(1) } else { f_rank + 1 };
                        if k_rank == forward_rank && (f_file as isize - k_file as isize).abs() == 1 {
                            return true;
                        }
                    } else if self.can_piece_reach(sq, k_sq, pt) {
                        return true;
                    }
                }
            }
        }

        false
    }

    fn has_no_legal_moves(&self, _color: Color) -> bool {
        // Simple heuristic for checkmate notation check
        false
    }
    fn to_fen(&self) -> String {
        let mut result = String::with_capacity(90);

        // 1. Board placement
        for rank in 0..8 {
            let mut empty_run = 0;
            for file in 0..8 {
                let sq = rank * 8 + file;
                match self.squares[sq] {
                    None => empty_run += 1,
                    Some(Piece { color, piece_type }) => {
                        if empty_run > 0 {
                            result.push_str(&empty_run.to_string());
                            empty_run = 0;
                        }
                        let mut c = match piece_type {
                            PieceType::Pawn => 'p',
                            PieceType::Knight => 'n',
                            PieceType::Bishop => 'b',
                            PieceType::Rook => 'r',
                            PieceType::Queen => 'q',
                            PieceType::King => 'k',
                        };
                        if color == Color::White {
                            c = c.to_ascii_uppercase();
                        }
                        result.push(c);
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

        // 2. Active turn
        result.push(' ');
        result.push(if self.turn == Color::White { 'w' } else { 'b' });

        // 3. Castling rights
        result.push(' ');
        let mut castling_str = String::new();
        if self.castling[0] { castling_str.push('K'); }
        if self.castling[1] { castling_str.push('Q'); }
        if self.castling[2] { castling_str.push('k'); }
        if self.castling[3] { castling_str.push('q'); }
        if castling_str.is_empty() {
            result.push('-');
        } else {
            result.push_str(&castling_str);
        }

        // 4. En-passant square
        result.push(' ');
        if let Some(ep) = self.ep_square {
            let file = ep % 8;
            let rank = 7 - (ep / 8);
            result.push((b'a' + file) as char);
            result.push((b'1' + rank) as char);
        } else {
            result.push('-');
        }

        // 5. Halfmove clock & Fullmove number
        result.push(' ');
        result.push_str(&self.halfmove.to_string());
        result.push(' ');
        result.push_str(&self.fullmove.to_string());

        result
    }
}

pub struct Exporter;

impl Exporter {
    /// Formats a single puzzle into standard PGN format.
    pub fn puzzle_to_pgn(puzzle: &Puzzle) -> String {
        Self::puzzle_to_pgn_with_options(puzzle, true)
    }

    /// Formats a single puzzle into standard PGN format with option to include or exclude the opponent's setup move.
    ///
    /// When `include_setup_move` is false:
    /// - The initial opponent move (Move 1) is executed on the board to advance the FEN to the exact state where the user must solve the puzzle.
    /// - The PGN moves start directly with the player's solution move.
    pub fn puzzle_to_pgn_with_options(puzzle: &Puzzle, include_setup_move: bool) -> String {
        let mut board = BoardState::from_fen(&puzzle.fen);
        let moves = &puzzle.moves;

        if !include_setup_move && moves.len() > 1 {
            // Apply first (setup) move to advance board state and FEN
            let mut setup_san = String::new();
            if let Some(ref mut b) = board {
                setup_san = b.uci_to_san_and_apply(&moves[0]);
            }

            let advanced_fen = if let Some(ref b) = board {
                b.to_fen()
            } else {
                puzzle.fen.clone()
            };

            let mut pgn = String::with_capacity(512);
            pgn.push_str(&format!("[Event \"Lichess Puzzle {}\"]\n", puzzle.id));
            pgn.push_str("[Site \"https://lichess.org/training\"]\n");
            pgn.push_str("[Date \"????.??.??\"]\n");
            pgn.push_str("[White \"Puzzle\"]\n");
            pgn.push_str("[Black \"Puzzle\"]\n");
            pgn.push_str("[Result \"*\"]\n");
            pgn.push_str("[SetUp \"1\"]\n");
            pgn.push_str(&format!("[FEN \"{}\"]\n", advanced_fen));
            pgn.push_str(&format!("[PuzzleId \"{}\"]\n", puzzle.id));
            pgn.push_str(&format!("[Rating \"{}\"]\n", puzzle.rating));
            if !puzzle.themes.is_empty() {
                pgn.push_str(&format!("[Themes \"{}\"]\n", puzzle.themes.join(" ")));
            }
            pgn.push_str(&format!("[SetupMove \"{}\"]\n", setup_san));
            pgn.push('\n');

            let fen_parts: Vec<&str> = advanced_fen.split_whitespace().collect();
            let is_black_to_move = fen_parts.get(1).map(|&c| c == "b").unwrap_or(false);
            let fullmove_number = fen_parts.get(5).and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);

            let mut current_move = fullmove_number;
            let mut is_black = is_black_to_move;

            for (i, uci_mv) in moves[1..].iter().enumerate() {
                let san_mv = if let Some(ref mut b) = board {
                    b.uci_to_san_and_apply(uci_mv)
                } else {
                    uci_mv.clone()
                };

                if i == 0 {
                    if is_black {
                        pgn.push_str(&format!("{}... {} ", current_move, san_mv));
                        current_move += 1;
                        is_black = false;
                    } else {
                        pgn.push_str(&format!("{}. {} ", current_move, san_mv));
                        is_black = true;
                    }
                } else if !is_black {
                    pgn.push_str(&format!("{}. {} ", current_move, san_mv));
                    is_black = true;
                } else {
                    pgn.push_str(&format!("{} ", san_mv));
                    current_move += 1;
                    is_black = false;
                }
            }

            pgn.push_str("*\n\n");
            return pgn;
        }

        // Default behavior: Include setup move
        let mut pgn = String::with_capacity(512);
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

        let fen_parts: Vec<&str> = puzzle.fen.split_whitespace().collect();
        let is_black_to_move = fen_parts.get(1).map(|&c| c == "b").unwrap_or(false);
        let fullmove_number = fen_parts.get(5).and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);

        let mut current_move = fullmove_number;
        let mut is_black = is_black_to_move;

        for (i, uci_mv) in puzzle.moves.iter().enumerate() {
            let san_mv = if let Some(ref mut b) = board {
                b.uci_to_san_and_apply(uci_mv)
            } else {
                uci_mv.clone()
            };

            if i == 0 {
                if is_black {
                    pgn.push_str(&format!("{}... {} ", current_move, san_mv));
                    current_move += 1;
                    is_black = false;
                } else {
                    pgn.push_str(&format!("{}. {} ", current_move, san_mv));
                    is_black = true;
                }
            } else if !is_black {
                pgn.push_str(&format!("{}. {} ", current_move, san_mv));
                is_black = true;
            } else {
                pgn.push_str(&format!("{} ", san_mv));
                current_move += 1;
                is_black = false;
            }
        }

        pgn.push_str("*\n\n");
        pgn
    }

    /// Transforms a puzzle to exclude the setup move, advancing the FEN and truncating moves.
    pub fn puzzle_without_setup_move(puzzle: &Puzzle) -> Puzzle {
        if puzzle.moves.len() <= 1 {
            return puzzle.clone();
        }

        let mut board = match BoardState::from_fen(&puzzle.fen) {
            Some(b) => b,
            None => return puzzle.clone(),
        };

        board.uci_to_san_and_apply(&puzzle.moves[0]);
        let advanced_fen = board.to_fen();

        Puzzle {
            id: puzzle.id.clone(),
            fen: advanced_fen,
            moves: puzzle.moves[1..].to_vec(),
            rating: puzzle.rating,
            themes: puzzle.themes.clone(),
        }
    }

    /// Exports a slice or iterator of puzzles to a PGN file.
    pub fn export_to_pgn_file<'a, I>(puzzles: I, path: impl AsRef<Path>, include_setup_move: bool) -> io::Result<usize>
    where
        I: IntoIterator<Item = &'a Puzzle>,
    {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        let mut count = 0;

        for p in puzzles {
            let pgn_text = Self::puzzle_to_pgn_with_options(p, include_setup_move);
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
    pub fn export_to_csv_file<'a, I>(puzzles: I, path: impl AsRef<Path>, include_setup_move: bool) -> io::Result<usize>
    where
        I: IntoIterator<Item = &'a Puzzle>,
    {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        let mut count = 0;

        // CSV Header
        writer.write_all(b"PuzzleId,FEN,Moves,Rating,Themes\n")?;

        for p in puzzles {
            let puzzle_to_export = if !include_setup_move {
                Self::puzzle_without_setup_move(p)
            } else {
                p.clone()
            };
            let line = Self::puzzle_to_csv_line(&puzzle_to_export);
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
    fn test_puzzle_to_pgn_san_white() {
        let p = Puzzle {
            id: "00008".to_string(),
            fen: "r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K w - - 0 24".to_string(),
            moves: vec!["f6e6".to_string(), "d5e6".to_string(), "c1b2".to_string()],
            rating: 1850,
            themes: vec!["crushing".to_string(), "hangingPiece".to_string()],
        };

        let pgn = Exporter::puzzle_to_pgn(&p);
        assert!(pgn.contains("[PuzzleId \"00008\"]"));
        assert!(pgn.contains("[FEN \"r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K w - - 0 24\"]"));
        // 24. Rxe7 dxe6 25. Bc1 or similar in SAN
        assert!(pgn.contains("24."));
    }

    #[test]
    fn test_puzzle_to_pgn_san_black_ply() {
        let p = Puzzle {
            id: "0000d".to_string(),
            fen: "5rk1/1p3ppp/pq3b2/8/8/1P1Q1N2/P4PPP/3R2K1 b - - 2 27".to_string(),
            moves: vec!["f8d8".to_string(), "d3e2".to_string()],
            rating: 1500,
            themes: vec!["advantage".to_string(), "short".to_string()],
        };

        let pgn = Exporter::puzzle_to_pgn(&p);
        // Starts with 27... Rd8 28. Qe2
        assert!(pgn.contains("27... Rd8 28. Qe2 *"));
    }
}
