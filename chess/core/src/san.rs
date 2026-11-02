//! Standard algebraic notation (SAN): "e4", "Nbd7", "exd6", "O-O", "e8=Q#".

use core::fmt;

use crate::board::{Board, Color, Piece, PieceKind};
use crate::movegen::{Move, MoveKind};

/// A legal move written in SAN, made by [`Board::san`].
pub struct San<'a> {
    board: &'a Board,
    m: Move,
}

impl Board {
    /// SAN of `m`, which must come from `legal_moves()`. Piece letters are
    /// English (K, Q, R, B, N); a UI that shows figurines or other letters
    /// replaces those uppercase letters.
    pub fn san(&self, m: Move) -> San<'_> {
        San { board: self, m }
    }
}

impl fmt::Display for San<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let San { board, m } = *self;
        let (from, to) = (m.from(), m.to());
        let Some(piece) = board.piece_at(from) else {
            return Ok(());
        };

        match m.kind() {
            MoveKind::CastleKingside => f.write_str("O-O")?,
            MoveKind::CastleQueenside => f.write_str("O-O-O")?,
            _ if piece.kind == PieceKind::Pawn => {
                if board.piece_at(to).is_some() || m.kind() == MoveKind::EnPassant {
                    write!(f, "{}x", (b'a' + from.file()) as char)?;
                }
                write!(f, "{to}")?;
                if let Some(kind) = m.promotion() {
                    write!(f, "={}", letter(kind))?;
                }
            }
            _ => {
                write!(f, "{}", letter(piece.kind))?;
                // Name the file, the rank or both, whichever tells this piece
                // apart from others of its kind that can reach the same square.
                let moves = board.legal_moves();
                let rivals = || {
                    moves
                        .iter()
                        .filter(move |other| other.to() == to && other.from() != from)
                        .filter(|other| board.piece_at(other.from()) == Some(piece))
                };
                if rivals().next().is_some() {
                    let same_file = rivals().any(|other| other.from().file() == from.file());
                    let same_rank = rivals().any(|other| other.from().rank() == from.rank());
                    if !same_file {
                        write!(f, "{}", (b'a' + from.file()) as char)?;
                    } else if !same_rank {
                        write!(f, "{}", from.rank() + 1)?;
                    } else {
                        write!(f, "{from}")?;
                    }
                }
                if board.piece_at(to).is_some() {
                    f.write_str("x")?;
                }
                write!(f, "{to}")?;
            }
        }

        let next = board.play(m);
        if next.in_check() {
            f.write_str(if next.legal_moves().is_empty() {
                "#"
            } else {
                "+"
            })?;
        }
        Ok(())
    }
}

fn letter(kind: PieceKind) -> char {
    Piece::new(Color::White, kind).fen_char()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::STARTPOS_FEN;
    use std::string::ToString;

    fn san(fen: &str, uci: &str) -> std::string::String {
        let board = Board::from_fen(fen).unwrap();
        let m = board
            .parse_uci(uci)
            .unwrap_or_else(|| panic!("{uci} is not legal in {fen}"));
        board.san(m).to_string()
    }

    #[test]
    fn writes_pawn_and_piece_moves() {
        assert_eq!(san(STARTPOS_FEN, "e2e4"), "e4");
        assert_eq!(san(STARTPOS_FEN, "g1f3"), "Nf3");
        assert_eq!(
            san(
                "rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 2",
                "e4d5"
            ),
            "exd5"
        );
        assert_eq!(
            san(
                "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
                "e5d6"
            ),
            "exd6"
        );
        assert_eq!(san("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", "e1g1"), "O-O");
        assert_eq!(san("r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1", "e8c8"), "O-O-O");
    }

    #[test]
    fn writes_promotions_checks_and_mates() {
        assert_eq!(san("8/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7e8q"), "e8=Q");
        assert_eq!(san("3r4/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7d8n"), "exd8=N");
        assert_eq!(san("4k3/8/8/8/8/8/8/R3K3 w - - 0 1", "a1a8"), "Ra8+");
        assert_eq!(
            san(
                "rnbqkbnr/pppp1ppp/8/4p3/6P1/5P2/PPPPP2P/RNBQKBNR b KQkq - 0 2",
                "d8h4"
            ),
            "Qh4#"
        );
    }

    #[test]
    fn disambiguates_by_file_then_rank_then_square() {
        // Knights on b1 and f3 both reach d2: the file tells them apart.
        assert_eq!(san("4k3/8/8/8/8/5N2/8/1N2K3 w - - 0 1", "b1d2"), "Nbd2");
        // Rooks on a1 and a5 share the file: the rank tells them apart.
        assert_eq!(san("4k3/8/8/R7/8/8/8/R3K3 w - - 0 1", "a1a3"), "R1a3");
        // Queens on a1, a3 and c1 all reach b2: a1 shares a file and a rank with them.
        assert_eq!(san("4k3/8/8/8/8/Q7/8/Q1Q1K3 w - - 0 1", "a1b2"), "Qa1b2");
        // The knight on e4 is pinned, so only g4's knight can go to f2.
        assert_eq!(san("4k3/4r3/8/8/4N1N1/8/8/4K3 w - - 0 1", "g4f2"), "Nf2");
    }
}
