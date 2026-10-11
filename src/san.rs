//! Robust SAN (Standard Algebraic Notation) parser to PackedMove.
//!
//! Converts PGN moves into 16-bit PackedMove representation by simulating
//! board state from the initial chess position.

use crate::moves::PackedMove;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    White,
    Black,
}

impl Color {
    #[inline(always)]
    pub fn opponent(&self) -> Self {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PieceType {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece {
    pub color: Color,
    pub piece_type: PieceType,
}

#[derive(Clone, Debug)]
pub struct Board {
    pub squares: [Option<Piece>; 64],
    pub turn: Color,
    pub castling: [bool; 4], // [W_K, W_Q, B_k, B_q]
    pub ep_square: Option<u8>,
}

impl Default for Board {
    fn default() -> Self {
        Self::startpos()
    }
}

impl Board {
    /// Creates the standard starting chess position.
    pub fn startpos() -> Self {
        let mut squares = [None; 64];

        // White pieces (rank 0 = rank 1)
        squares[0] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Rook,
        });
        squares[1] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Knight,
        });
        squares[2] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Bishop,
        });
        squares[3] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Queen,
        });
        squares[4] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::King,
        });
        squares[5] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Bishop,
        });
        squares[6] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Knight,
        });
        squares[7] = Some(Piece {
            color: Color::White,
            piece_type: PieceType::Rook,
        });

        // White pawns (rank 1 = rank 2)
        for f in 0..8 {
            squares[8 + f] = Some(Piece {
                color: Color::White,
                piece_type: PieceType::Pawn,
            });
        }

        // Black pawns (rank 6 = rank 7)
        for f in 0..8 {
            squares[48 + f] = Some(Piece {
                color: Color::Black,
                piece_type: PieceType::Pawn,
            });
        }

        // Black pieces (rank 7 = rank 8)
        squares[56] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Rook,
        });
        squares[57] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Knight,
        });
        squares[58] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Bishop,
        });
        squares[59] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Queen,
        });
        squares[60] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::King,
        });
        squares[61] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Bishop,
        });
        squares[62] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Knight,
        });
        squares[63] = Some(Piece {
            color: Color::Black,
            piece_type: PieceType::Rook,
        });

        Self {
            squares,
            turn: Color::White,
            castling: [true, true, true, true],
            ep_square: None,
        }
    }

    #[inline(always)]
    fn is_clear_path(&self, from_sq: usize, to_sq: usize) -> bool {
        let f_r = (from_sq / 8) as isize;
        let f_f = (from_sq % 8) as isize;
        let t_r = (to_sq / 8) as isize;
        let t_f = (to_sq % 8) as isize;

        let dr = (t_r - f_r).signum();
        let df = (t_f - f_f).signum();

        let mut curr_r = f_r + dr;
        let mut curr_f = f_f + df;

        while curr_r != t_r || curr_f != t_f {
            let sq = (curr_r * 8 + curr_f) as usize;
            if self.squares[sq].is_some() {
                return false;
            }
            curr_r += dr;
            curr_f += df;
        }

        true
    }

    fn can_piece_attack(&self, from_sq: usize, to_sq: usize, pt: PieceType) -> bool {
        let f_r = (from_sq / 8) as isize;
        let f_f = (from_sq % 8) as isize;
        let t_r = (to_sq / 8) as isize;
        let t_f = (to_sq % 8) as isize;

        let dr = (t_r - f_r).abs();
        let df = (t_f - f_f).abs();

        match pt {
            PieceType::Knight => (dr == 1 && df == 2) || (dr == 2 && df == 1),
            PieceType::Bishop => dr == df && self.is_clear_path(from_sq, to_sq),
            PieceType::Rook => (dr == 0 || df == 0) && self.is_clear_path(from_sq, to_sq),
            PieceType::Queen => {
                (dr == df || dr == 0 || df == 0) && self.is_clear_path(from_sq, to_sq)
            }
            PieceType::King => dr <= 1 && df <= 1,
            PieceType::Pawn => false, // Handled separately
        }
    }

    pub fn is_in_check(&self, color: Color) -> bool {
        let king_sq = (0..64).find(|&sq| {
            matches!(self.squares[sq], Some(Piece { color: c, piece_type: PieceType::King }) if c == color)
        });

        let k_sq = match king_sq {
            Some(sq) => sq,
            None => return false,
        };

        let opp = color.opponent();
        let k_r = (k_sq / 8) as isize;
        let k_f = (k_sq % 8) as isize;

        for sq in 0..64 {
            if let Some(Piece {
                color: c,
                piece_type: pt,
            }) = self.squares[sq]
            {
                if c == opp {
                    if pt == PieceType::Pawn {
                        let p_r = (sq / 8) as isize;
                        let p_f = (sq % 8) as isize;
                        let forward = if opp == Color::White { 1 } else { -1 };
                        if k_r == p_r + forward && (k_f - p_f).abs() == 1 {
                            return true;
                        }
                    } else if self.can_piece_attack(sq, k_sq, pt) {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Simulates a move and checks if it is legal (does not leave king in check).
    fn is_legal_move(&self, from_sq: usize, to_sq: usize, promo: Option<PieceType>) -> bool {
        let mut cloned = self.clone();
        cloned.apply_move_unchecked(from_sq, to_sq, promo);
        !cloned.is_in_check(self.turn)
    }

    /// Applies a move directly without validating legality.
    pub fn apply_move_unchecked(&mut self, from_sq: usize, to_sq: usize, promo: Option<PieceType>) {
        let piece = match self.squares[from_sq].take() {
            Some(p) => p,
            None => return,
        };

        let mut dest_piece = piece;
        if let Some(p) = promo {
            dest_piece.piece_type = p;
        }

        // En-passant capture
        if piece.piece_type == PieceType::Pawn && Some(to_sq as u8) == self.ep_square {
            let cap_sq = if piece.color == Color::White {
                to_sq.wrapping_sub(8)
            } else {
                to_sq + 8
            };
            if cap_sq < 64 {
                self.squares[cap_sq] = None;
            }
        }

        // King castling rook moves & disable castling
        if piece.piece_type == PieceType::King {
            if piece.color == Color::White {
                self.castling[0] = false;
                self.castling[1] = false;
                if from_sq == 4 && to_sq == 6 {
                    self.squares[5] = self.squares[7].take();
                } else if from_sq == 4 && to_sq == 2 {
                    self.squares[3] = self.squares[0].take();
                }
            } else {
                self.castling[2] = false;
                self.castling[3] = false;
                if from_sq == 60 && to_sq == 62 {
                    self.squares[61] = self.squares[63].take();
                } else if from_sq == 60 && to_sq == 58 {
                    self.squares[59] = self.squares[56].take();
                }
            }
        }

        // Rook moves disable corresponding castling
        if from_sq == 0 {
            self.castling[1] = false;
        }
        if from_sq == 7 {
            self.castling[0] = false;
        }
        if from_sq == 56 {
            self.castling[3] = false;
        }
        if from_sq == 63 {
            self.castling[2] = false;
        }
        if to_sq == 0 {
            self.castling[1] = false;
        }
        if to_sq == 7 {
            self.castling[0] = false;
        }
        if to_sq == 56 {
            self.castling[3] = false;
        }
        if to_sq == 63 {
            self.castling[2] = false;
        }

        self.squares[to_sq] = Some(dest_piece);

        // Update en-passant square
        if piece.piece_type == PieceType::Pawn {
            let diff = (to_sq as isize - from_sq as isize).abs();
            if diff == 16 {
                let ep = (from_sq + to_sq) / 2;
                self.ep_square = Some(ep as u8);
            } else {
                self.ep_square = None;
            }
        } else {
            self.ep_square = None;
        }

        self.turn = self.turn.opponent();
    }

    /// Parses a single SAN move token (e.g. "e4", "Nf3", "Bxc6+", "O-O", "e8=Q#")
    /// returns the `PackedMove` and applies it to `self`.
    pub fn parse_san_and_apply(&mut self, san_raw: &str) -> Option<PackedMove> {
        let san = san_raw.trim().trim_end_matches(['+', '#', '!', '?']);
        if san.is_empty() {
            return None;
        }

        // 1. Castling
        if san == "O-O" || san == "0-0" {
            let (from_sq, to_sq) = match self.turn {
                Color::White => (4, 6),
                Color::Black => (60, 62),
            };
            self.apply_move_unchecked(from_sq, to_sq, None);
            return Some(PackedMove::new(from_sq as u8, to_sq as u8, 0));
        }
        if san == "O-O-O" || san == "0-0-0" {
            let (from_sq, to_sq) = match self.turn {
                Color::White => (4, 2),
                Color::Black => (60, 58),
            };
            self.apply_move_unchecked(from_sq, to_sq, None);
            return Some(PackedMove::new(from_sq as u8, to_sq as u8, 0));
        }

        // 2. Promotion check (e.g. "e8=Q", "e8Q", "exd8=Q")
        let (san_clean, promo, promo_code) = if let Some(idx) = san.find('=') {
            let pt_char = san.as_bytes().get(idx + 1).copied()?;
            let (pt, code) = match pt_char {
                b'Q' | b'q' => (PieceType::Queen, 1),
                b'R' | b'r' => (PieceType::Rook, 2),
                b'B' | b'b' => (PieceType::Bishop, 3),
                b'N' | b'n' => (PieceType::Knight, 4),
                _ => return None,
            };
            (&san[..idx], Some(pt), code)
        } else if san.len() >= 4
            && san.as_bytes()[san.len() - 1].is_ascii_alphabetic()
            && !san.as_bytes()[san.len() - 2].is_ascii_alphabetic()
        {
            // Implicit promotion without '=' like e8Q
            let pt_char = san.as_bytes()[san.len() - 1];
            let (pt, code) = match pt_char {
                b'Q' | b'q' => (PieceType::Queen, 1),
                b'R' | b'r' => (PieceType::Rook, 2),
                b'B' | b'b' => (PieceType::Bishop, 3),
                b'N' | b'n' => (PieceType::Knight, 4),
                _ => (PieceType::Queen, 0),
            };
            if code > 0 {
                (&san[..san.len() - 1], Some(pt), code)
            } else {
                (san, None, 0)
            }
        } else {
            (san, None, 0)
        };

        let bytes = san_clean.as_bytes();
        if bytes.len() < 2 {
            return None;
        }

        // Destination square is the last 2 characters of san_clean
        let to_file_b = bytes[bytes.len() - 2];
        let to_rank_b = bytes[bytes.len() - 1];

        if !(b'a'..=b'h').contains(&to_file_b) || !(b'1'..=b'8').contains(&to_rank_b) {
            return None;
        }

        let to_file = (to_file_b - b'a') as usize;
        let to_rank = (to_rank_b - b'1') as usize;
        let to_sq = to_rank * 8 + to_file;

        let first_char = bytes[0];
        let is_piece = matches!(first_char, b'N' | b'B' | b'R' | b'Q' | b'K');

        if !is_piece {
            // 3. Pawn Move
            let from_sq = if bytes.contains(&b'x') || (bytes.len() >= 3 && bytes[0] != to_file_b) {
                // Pawn capture (e.g. "exd5", "ed5")
                let from_file = (bytes[0].checked_sub(b'a')?) as usize;
                if from_file > 7 {
                    return None;
                }
                let from_rank = match self.turn {
                    Color::White => to_rank.checked_sub(1)?,
                    Color::Black => to_rank + 1,
                };
                let f_sq = from_rank * 8 + from_file;
                if f_sq >= 64 {
                    return None;
                }
                f_sq
            } else {
                // Pawn push (e.g. "e4", "d3")
                let from_file = to_file;
                match self.turn {
                    Color::White => {
                        let one_step = to_rank.checked_sub(1)? * 8 + from_file;
                        if matches!(
                            self.squares[one_step],
                            Some(Piece {
                                color: Color::White,
                                piece_type: PieceType::Pawn
                            })
                        ) {
                            one_step
                        } else if to_rank == 3 {
                            let two_step = 1 * 8 + from_file;
                            if matches!(
                                self.squares[two_step],
                                Some(Piece {
                                    color: Color::White,
                                    piece_type: PieceType::Pawn
                                })
                            ) && self.squares[2 * 8 + from_file].is_none()
                            {
                                two_step
                            } else {
                                return None;
                            }
                        } else {
                            return None;
                        }
                    }
                    Color::Black => {
                        let one_step = (to_rank + 1) * 8 + from_file;
                        if one_step < 64
                            && matches!(
                                self.squares[one_step],
                                Some(Piece {
                                    color: Color::Black,
                                    piece_type: PieceType::Pawn
                                })
                            )
                        {
                            one_step
                        } else if to_rank == 4 {
                            let two_step = 6 * 8 + from_file;
                            if matches!(
                                self.squares[two_step],
                                Some(Piece {
                                    color: Color::Black,
                                    piece_type: PieceType::Pawn
                                })
                            ) && self.squares[5 * 8 + from_file].is_none()
                            {
                                two_step
                            } else {
                                return None;
                            }
                        } else {
                            return None;
                        }
                    }
                }
            };

            if self.is_legal_move(from_sq, to_sq, promo) {
                self.apply_move_unchecked(from_sq, to_sq, promo);
                return Some(PackedMove::new(from_sq as u8, to_sq as u8, promo_code));
            }
            return None;
        }

        // 4. Piece Move (N, B, R, Q, K)
        let pt = match first_char {
            b'N' => PieceType::Knight,
            b'B' => PieceType::Bishop,
            b'R' => PieceType::Rook,
            b'Q' => PieceType::Queen,
            b'K' => PieceType::King,
            _ => return None,
        };

        // Parse disambiguation characters between first_char and destination
        let disambig_slice = &bytes[1..bytes.len() - 2];
        let mut req_file = None;
        let mut req_rank = None;

        for &b in disambig_slice {
            if b == b'x' {
                continue;
            }
            if (b'a'..=b'h').contains(&b) {
                req_file = Some((b - b'a') as usize);
            } else if (b'1'..=b'8').contains(&b) {
                req_rank = Some((b - b'1') as usize);
            }
        }

        let mut candidate_from = None;
        for sq in 0..64 {
            if let Some(Piece {
                color: c,
                piece_type: piece_t,
            }) = self.squares[sq]
            {
                if c == self.turn && piece_t == pt {
                    let f_file = sq % 8;
                    let f_rank = sq / 8;
                    if let Some(rf) = req_file {
                        if f_file != rf {
                            continue;
                        }
                    }
                    if let Some(rr) = req_rank {
                        if f_rank != rr {
                            continue;
                        }
                    }
                    if self.can_piece_attack(sq, to_sq, pt) && self.is_legal_move(sq, to_sq, None) {
                        candidate_from = Some(sq);
                        break;
                    }
                }
            }
        }

        let from_sq = candidate_from?;
        self.apply_move_unchecked(from_sq, to_sq, None);
        Some(PackedMove::new(from_sq as u8, to_sq as u8, 0))
    }
}

/// Parses a full game PGN movetext into a vector of PackedMove.
/// Strips PGN tags (`[Tag "Value"]`), comments (`{ ... }`), variations (`( ... )`), move numbers (`1.`, `2...`), and results.
pub fn parse_pgn_moves(pgn_input: &str) -> (Vec<PackedMove>, Option<&'static str>) {
    let mut board = Board::startpos();
    let mut moves = Vec::new();
    let mut result = None;

    // 1. Strip PGN tag lines
    let mut movetext_only = String::with_capacity(pgn_input.len());
    for line in pgn_input.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            continue;
        }
        movetext_only.push_str(line);
        movetext_only.push(' ');
    }

    // 2. Remove comments in braces and RAV variations in parentheses
    let mut clean_text = String::with_capacity(movetext_only.len());
    let mut brace_depth = 0;
    let mut paren_depth = 0;

    for c in movetext_only.chars() {
        match c {
            '{' => brace_depth += 1,
            '}' => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                }
            }
            '(' => paren_depth += 1,
            ')' => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
            }
            _ => {
                if brace_depth == 0 && paren_depth == 0 {
                    clean_text.push(c);
                }
            }
        }
    }

    for token in clean_text.split_whitespace() {
        if token.is_empty() {
            continue;
        }

        // Check result tokens
        if token == "1-0" {
            result = Some("1-0");
            break;
        } else if token == "0-1" {
            result = Some("0-1");
            break;
        } else if token == "1/2-1/2" {
            result = Some("1/2-1/2");
            break;
        } else if token == "*" {
            result = Some("*");
            break;
        }

        // Skip move numbers (e.g. "1.", "12...", "42.")
        if token.ends_with('.')
            || token.contains("...")
            || token.chars().all(|c| c.is_ascii_digit() || c == '.')
        {
            continue;
        }

        if let Some(pm) = board.parse_san_and_apply(token) {
            moves.push(pm);
        } else {
            // Stop if an unparseable move is encountered
            break;
        }
    }

    (moves, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_knvuotqb() {
        let pgn = "1. b3 d5 2. Bb2 c5 3. e3 Nc6 4. Bb5 a6 5. Bxc6+ bxc6 6. c4 dxc4 7. bxc4 Be6 8. d3 Nf6 9. Nf3 Bg4 10. h3 Bxf3 11. gxf3 e6 12. Ke2 Be7 13. Nc3 O-O 14. Qa4 Qc8 15. Rag1 g6 16. f4 Nh5 17. Ne4 Rb8 18. Bc3 Rd8 19. Rg2 f6 20. Ba5 Rd7 21. h4 f5 22. Ng5 Bxg5 23. Rxg5 Rb2+ 24. Bd2 Qb7 25. Rhg1 Kf7 26. f3 Rxd2+ 27. Kxd2 Qb2+ 28. Qc2 Qb4+ 29. Ke2 Qb8 30. Rb1 Qc7 31. Qa4 Qd6 32. Rb3 h6 33. Rg1 Qe7 34. Qxc6 e5 35. fxe5 Qxe5 36. Qxd7+ 1-0";

        let (moves, res) = parse_pgn_moves(pgn);
        assert_eq!(res, Some("1-0"));
        assert_eq!(moves.len(), 71); // 35 full moves + 1 half move = 71 plies
        assert_eq!(moves[0].to_uci(), "b2b3");
        assert_eq!(moves[1].to_uci(), "d7d5");
        assert_eq!(moves[2].to_uci(), "c1b2");
        assert_eq!(moves[3].to_uci(), "c7c5");
        assert_eq!(moves[70].to_uci(), "c6d7"); // Qxd7+
    }

    #[test]
    fn test_parse_en_passant_and_castling() {
        let pgn = "1. e4 c5 2. Nf3 d6 3. d4 cxd4 4. Nxd4 Nf6 5. Nc3 a6 6. Be3 e5 7. Nb3 Be6 8. Qd2 Nbd7 9. O-O-O Be7 10. f3 h5 11. Kb1 Rc8 12. h3 h4 13. f4 b5 14. f5 Bc4 15. Bd3 Nb6 16. Qf2 Na4 17. Ne2 d5 18. exd5 e4 19. d6 exd3 20. dxe7 Qxe7 21. cxd3 Bxb3 22. axb3 Nc5 23. Bxc5 Rxc5 24. d4 Rc8 25. Nc3 O-O 26. Qxh4 Qb4 27. Rd3 Qxb3 28. Rg3 Nh7 29. f6 g6 30. Qh6 Nxf6 31. Rf1 Rxc3 32. Rxc3 Qe6 33. Rcf3 Qe4+ 34. Ka1 Nd5 35. Rf5 Ne3 36. R5f4 Nc2+ 37. Ka2 Qd5+ 38. Kb1 Qb3 39. Rh4 Na3+ 40. Ka1 Nc2+ 41. Kb1 Na3+ 42. Ka1 Nc2+ 1/2-1/2";
        let (moves, res) = parse_pgn_moves(pgn);
        assert_eq!(res, Some("1/2-1/2"));
        assert_eq!(moves.len(), 84);
        assert_eq!(moves[16].to_uci(), "e1c1"); // O-O-O
        assert_eq!(moves[49].to_uci(), "e8g8"); // O-O
    }

    #[test]
    fn test_parse_promotion() {
        let pgn = "1. e4 d5 2. exd5 Nf6 3. d4 Nxd5 4. Nf3 Bg4 5. Be2 e6 6. O-O Be7 7. c4 Nb6 8. Nc3 O-O 9. b3 Nc6 10. Be3 Bf6 11. Ne4 Be7 12. h3 Bh5 13. Ng3 Bg6 14. Ne5 Nxe5 15. dxe5 Qxd1 16. Raxd1 Rfd8 17. f4 Rxd1 18. Rxd1 Rd8 19. Rxd8+ Bxd8 20. Bf3 c6 21. Ne4 Bxe4 22. Bxe4 f5 23. Bf3 a6 24. g4 g6 25. Kf2 Kf7 26. Ke2 Ke8 27. Kd3 Kd7 28. a4 Bc7 29. a5 Nc8 30. b4 Ne7 31. Bc5 Ng8 32. b5 axb5 33. cxb5 Bxa5 34. bxc6+ bxc6 35. gxf5 gxf5 36. Bh5 Ne7 37. Bxe7 Kxe7 38. Bf3 Kd7 39. Bh5 Be1 40. Ke2 Bg3 41. Kf3 Bh2 42. Ke3 c5 43. Be2 Kc6 44. Bc4 Kd7 45. Bb5+ Ke7 46. Bc4 Bg1+ 47. Kf3 Bd4 48. h4 h5 49. Bb5 Bc3 50. Ke2 Bb2 51. Ke1 Bc1 52. Be2 Bxf4 53. Bxh5 Bxe5 54. Be2 Bg3+ 55. Kd2 Bxh4 56. Kd3 Kd6 57. Kc4 e5 58. Bh5 e4 59. Bg6 Ke5 60. Kxc5 f4 61. Kc4 f3 62. Kc3 Kf4 63. Kd2 e3+ 64. Kd3 e2 65. Kd2 e1=Q+ 0-1";
        let (moves, res) = parse_pgn_moves(pgn);
        assert_eq!(res, Some("0-1"));
        assert_eq!(moves.len(), 130);
        let last = moves.last().unwrap();
        assert_eq!(last.to_uci(), "e2e1q");
    }
}
