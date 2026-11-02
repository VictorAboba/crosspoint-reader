//! Move generation, making moves, game outcomes and perft.
//!
//! Moves are generated pseudo-legally on the mailbox board and kept only if
//! the mover's king is safe once the move is played on a copy. That is simple
//! and hard to get wrong; perft against published totals guards it, and the
//! engine can move to something faster behind the same API later.

use core::fmt;
use core::ops::Deref;

use crate::board::{Board, CastlingRights, Color, Piece, PieceKind, Square};

const KNIGHT_DELTAS: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
const KING_DELTAS: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
const ROOK_DIRECTIONS: [(i8, i8); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
const BISHOP_DIRECTIONS: [(i8, i8); 4] = [(1, 1), (-1, 1), (-1, -1), (1, -1)];
const PROMOTIONS: [PieceKind; 4] = [
    PieceKind::Queen,
    PieceKind::Rook,
    PieceKind::Bishop,
    PieceKind::Knight,
];

/// What a move does besides taking a piece from one square to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    Normal,
    /// A pawn two squares forward; sets the en passant square.
    DoublePush,
    CastleKingside,
    CastleQueenside,
    EnPassant,
    Promotion(PieceKind),
}

/// A move packed into 16 bits: from square (bits 0-5), to square (6-11) and
/// kind (12-15). The packing is part of the C API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move(u16);

impl Move {
    const KIND_SHIFT: u16 = 12;

    pub(crate) const fn new(from: Square, to: Square, kind: MoveKind) -> Move {
        let code: u16 = match kind {
            MoveKind::Normal => 0,
            MoveKind::DoublePush => 1,
            MoveKind::CastleKingside => 2,
            MoveKind::CastleQueenside => 3,
            MoveKind::EnPassant => 4,
            MoveKind::Promotion(PieceKind::Knight) => 5,
            MoveKind::Promotion(PieceKind::Bishop) => 6,
            MoveKind::Promotion(PieceKind::Rook) => 7,
            MoveKind::Promotion(_) => 8,
        };
        Move(from.index() as u16 | (to.index() as u16) << 6 | code << Self::KIND_SHIFT)
    }

    pub const fn from_bits(bits: u16) -> Move {
        Move(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn from(self) -> Square {
        Square::from_index((self.0 & 63) as u8)
    }

    pub const fn to(self) -> Square {
        Square::from_index((self.0 >> 6 & 63) as u8)
    }

    pub const fn kind(self) -> MoveKind {
        match self.0 >> Self::KIND_SHIFT {
            1 => MoveKind::DoublePush,
            2 => MoveKind::CastleKingside,
            3 => MoveKind::CastleQueenside,
            4 => MoveKind::EnPassant,
            5 => MoveKind::Promotion(PieceKind::Knight),
            6 => MoveKind::Promotion(PieceKind::Bishop),
            7 => MoveKind::Promotion(PieceKind::Rook),
            8 => MoveKind::Promotion(PieceKind::Queen),
            _ => MoveKind::Normal,
        }
    }

    pub const fn promotion(self) -> Option<PieceKind> {
        match self.kind() {
            MoveKind::Promotion(piece) => Some(piece),
            _ => None,
        }
    }
}

/// UCI notation: "e2e4", "e7e8q". Castling is written as the king's move, "e1g1".
impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.from(), self.to())?;
        if let Some(piece) = self.promotion() {
            write!(f, "{}", Piece::new(Color::Black, piece).fen_char())?;
        }
        Ok(())
    }
}

/// No legal position has more than 218 moves; the margin covers positions a
/// FEN can describe but a game cannot reach.
pub const MAX_MOVES: usize = 256;

/// A fixed-capacity move list, so move generation needs no heap.
#[derive(Clone)]
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: usize,
}

impl MoveList {
    pub const fn new() -> MoveList {
        MoveList {
            moves: [Move(0); MAX_MOVES],
            len: 0,
        }
    }

    fn push(&mut self, m: Move) {
        if self.len < MAX_MOVES {
            self.moves[self.len] = m;
            self.len += 1;
        }
    }
}

impl Default for MoveList {
    fn default() -> Self {
        MoveList::new()
    }
}

impl Deref for MoveList {
    type Target = [Move];

    fn deref(&self) -> &[Move] {
        &self.moves[..self.len]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Checkmate {
        winner: Color,
    },
    Stalemate,
    /// 50 moves by each side without a capture or a pawn move.
    FiftyMoveRule,
    InsufficientMaterial,
    /// The same position three times; only [`crate::Game::outcome`] sees it.
    ThreefoldRepetition,
    /// The game holds [`crate::MAX_PLIES`] moves, all it can store; a draw.
    /// Only [`crate::Game::outcome`] sees it.
    MoveLimit,
}

impl Board {
    pub fn in_check(&self) -> bool {
        let us = self.side_to_move;
        self.is_attacked(self.kings[us as usize], us.opposite())
    }

    /// Whether a piece of `by` attacks `square`.
    pub fn is_attacked(&self, square: Square, by: Color) -> bool {
        let holds = |target: Option<Square>, kind: PieceKind| {
            target.is_some_and(|target| self.piece_at(target) == Some(Piece::new(by, kind)))
        };
        // Pawns attack diagonally forward, so look one rank behind `square` from their side.
        let behind = match by {
            Color::White => -1,
            Color::Black => 1,
        };
        if holds(square.offset(-1, behind), PieceKind::Pawn)
            || holds(square.offset(1, behind), PieceKind::Pawn)
        {
            return true;
        }
        if KNIGHT_DELTAS
            .iter()
            .any(|&(files, ranks)| holds(square.offset(files, ranks), PieceKind::Knight))
            || KING_DELTAS
                .iter()
                .any(|&(files, ranks)| holds(square.offset(files, ranks), PieceKind::King))
        {
            return true;
        }
        let slider_attacks = |directions: &[(i8, i8)], kind: PieceKind| {
            directions.iter().any(|&(files, ranks)| {
                self.first_piece(square, files, ranks).is_some_and(|piece| {
                    piece.color == by && (piece.kind == kind || piece.kind == PieceKind::Queen)
                })
            })
        };
        slider_attacks(&ROOK_DIRECTIONS, PieceKind::Rook)
            || slider_attacks(&BISHOP_DIRECTIONS, PieceKind::Bishop)
    }

    pub fn legal_moves(&self) -> MoveList {
        let mut list = MoveList::new();
        self.pseudo_legal_moves(&mut |m| {
            if self.is_legal_result(&self.play(m)) {
                list.push(m);
            }
        });
        list
    }

    /// Whether `next`, made by a pseudo-legal move from this position, leaves
    /// the mover's king safe.
    pub(crate) fn is_legal_result(&self, next: &Board) -> bool {
        let us = self.side_to_move;
        !next.is_attacked(next.kings[us as usize], us.opposite())
    }

    /// Finds the legal move written in UCI notation ("e2e4", "e7e8q").
    pub fn parse_uci(&self, uci: &str) -> Option<Move> {
        if uci.len() > 5 {
            return None;
        }
        let from = Square::parse(uci.get(0..2)?)?;
        let to = Square::parse(uci.get(2..4)?)?;
        let promotion = match uci.get(4..)? {
            "" => None,
            p => Some(
                p.chars()
                    .next()
                    .filter(char::is_ascii_lowercase)
                    .and_then(Piece::from_fen_char)?
                    .kind,
            ),
        };
        self.legal_moves()
            .iter()
            .copied()
            .find(|m| m.from() == from && m.to() == to && m.promotion() == promotion)
    }

    /// The board after `m`, which must come from `legal_moves()`.
    pub fn play(&self, m: Move) -> Board {
        let mut next = self.clone();
        next.apply(m);
        next
    }

    /// How the game has ended at this position, if it has. Draws by
    /// repetition need the game history, so they are left to the caller.
    pub fn outcome(&self) -> Option<Outcome> {
        if self.legal_moves().is_empty() {
            return Some(if self.in_check() {
                Outcome::Checkmate {
                    winner: self.side_to_move.opposite(),
                }
            } else {
                Outcome::Stalemate
            });
        }
        if self.halfmove_clock >= 100 {
            return Some(Outcome::FiftyMoveRule);
        }
        if self.insufficient_material() {
            return Some(Outcome::InsufficientMaterial);
        }
        None
    }

    fn first_piece(&self, from: Square, files: i8, ranks: i8) -> Option<Piece> {
        let mut square = from;
        while let Some(next) = square.offset(files, ranks) {
            if let Some(piece) = self.piece_at(next) {
                return Some(piece);
            }
            square = next;
        }
        None
    }

    /// Moves that follow the piece rules but may leave the king in check;
    /// [`Board::is_legal_result`] tells which ones do not.
    pub(crate) fn pseudo_legal_moves(&self, emit: &mut impl FnMut(Move)) {
        for from in Square::all() {
            let Some(piece) = self
                .piece_at(from)
                .filter(|piece| piece.color == self.side_to_move)
            else {
                continue;
            };
            match piece.kind {
                PieceKind::Pawn => self.pawn_moves(from, emit),
                PieceKind::Knight => self.step_moves(from, &KNIGHT_DELTAS, emit),
                PieceKind::Bishop => self.slide_moves(from, &BISHOP_DIRECTIONS, emit),
                PieceKind::Rook => self.slide_moves(from, &ROOK_DIRECTIONS, emit),
                PieceKind::Queen => {
                    self.slide_moves(from, &ROOK_DIRECTIONS, emit);
                    self.slide_moves(from, &BISHOP_DIRECTIONS, emit);
                }
                PieceKind::King => {
                    self.step_moves(from, &KING_DELTAS, emit);
                    self.castling_moves(from, emit);
                }
            }
        }
    }

    fn step_moves(&self, from: Square, deltas: &[(i8, i8)], emit: &mut impl FnMut(Move)) {
        for &(files, ranks) in deltas {
            if let Some(to) = from.offset(files, ranks)
                && self
                    .piece_at(to)
                    .is_none_or(|piece| piece.color != self.side_to_move)
            {
                emit(Move::new(from, to, MoveKind::Normal));
            }
        }
    }

    fn slide_moves(&self, from: Square, directions: &[(i8, i8)], emit: &mut impl FnMut(Move)) {
        for &(files, ranks) in directions {
            let mut square = from;
            while let Some(to) = square.offset(files, ranks) {
                match self.piece_at(to) {
                    None => emit(Move::new(from, to, MoveKind::Normal)),
                    Some(piece) => {
                        if piece.color != self.side_to_move {
                            emit(Move::new(from, to, MoveKind::Normal));
                        }
                        break;
                    }
                }
                square = to;
            }
        }
    }

    fn pawn_moves(&self, from: Square, emit: &mut impl FnMut(Move)) {
        let us = self.side_to_move;
        let (forward, start_rank, last_rank) = match us {
            Color::White => (1, 1, 7),
            Color::Black => (-1, 6, 0),
        };
        let advance = |to: Square, emit: &mut dyn FnMut(Move)| {
            if to.rank() == last_rank {
                for piece in PROMOTIONS {
                    emit(Move::new(from, to, MoveKind::Promotion(piece)));
                }
            } else {
                emit(Move::new(from, to, MoveKind::Normal));
            }
        };

        if let Some(one) = from.offset(0, forward)
            && self.piece_at(one).is_none()
        {
            advance(one, emit);
            if from.rank() == start_rank
                && let Some(two) = one.offset(0, forward)
                && self.piece_at(two).is_none()
            {
                emit(Move::new(from, two, MoveKind::DoublePush));
            }
        }
        for files in [-1, 1] {
            let Some(to) = from.offset(files, forward) else {
                continue;
            };
            match self.piece_at(to) {
                Some(piece) if piece.color != us => advance(to, emit),
                // The en passant square sits behind the enemy pawn that just made a double step.
                None if self.en_passant == Some(to)
                    && self.piece_at(Square::from_index(from.rank() * 8 + to.file()))
                        == Some(Piece::new(us.opposite(), PieceKind::Pawn)) =>
                {
                    emit(Move::new(from, to, MoveKind::EnPassant))
                }
                _ => {}
            }
        }
    }

    /// Castling moves from `king`. The king may not be in check or pass
    /// through an attacked square; the square it lands on is checked like
    /// any other king move.
    fn castling_moves(&self, king: Square, emit: &mut impl FnMut(Move)) {
        let us = self.side_to_move;
        let them = us.opposite();
        let (rank, kingside, queenside) = match us {
            Color::White => (
                0,
                CastlingRights::WHITE_KINGSIDE,
                CastlingRights::WHITE_QUEENSIDE,
            ),
            Color::Black => (
                7,
                CastlingRights::BLACK_KINGSIDE,
                CastlingRights::BLACK_QUEENSIDE,
            ),
        };
        let on_rank = |file: u8| Square::from_index(rank * 8 + file);
        if king != on_rank(4) || self.is_attacked(king, them) {
            return;
        }
        let rook = Some(Piece::new(us, PieceKind::Rook));
        let empty = |files: core::ops::Range<u8>| {
            files
                .into_iter()
                .all(|file| self.piece_at(on_rank(file)).is_none())
        };

        if self.castling.contains(kingside)
            && self.piece_at(on_rank(7)) == rook
            && empty(5..7)
            && !self.is_attacked(on_rank(5), them)
        {
            emit(Move::new(king, on_rank(6), MoveKind::CastleKingside));
        }
        if self.castling.contains(queenside)
            && self.piece_at(on_rank(0)) == rook
            && empty(1..4)
            && !self.is_attacked(on_rank(3), them)
        {
            emit(Move::new(king, on_rank(2), MoveKind::CastleQueenside));
        }
    }

    fn apply(&mut self, m: Move) {
        let us = self.side_to_move;
        let (from, to) = (m.from(), m.to());
        let Some(piece) = self.take(from) else { return };
        let captured = self.take(to);

        self.halfmove_clock = if piece.kind == PieceKind::Pawn || captured.is_some() {
            0
        } else {
            self.halfmove_clock.saturating_add(1)
        };
        self.en_passant = None;
        let mut placed = piece;
        match m.kind() {
            MoveKind::Normal => {}
            MoveKind::DoublePush => self.en_passant = Square::new((from.index() + to.index()) / 2),
            MoveKind::CastleKingside => self.move_rook(from.rank(), 7, 5),
            MoveKind::CastleQueenside => self.move_rook(from.rank(), 0, 3),
            MoveKind::EnPassant => {
                self.take(Square::from_index(from.rank() * 8 + to.file()));
            }
            MoveKind::Promotion(kind) => placed = Piece::new(us, kind),
        }
        self.squares[to.index() as usize] = Some(placed);
        if piece.kind == PieceKind::King {
            self.kings[us as usize] = to;
        }

        self.castling = self
            .castling
            .without(castling_lost(from) | castling_lost(to));
        if us == Color::Black {
            self.fullmove_number = self.fullmove_number.saturating_add(1);
        }
        self.side_to_move = us.opposite();
    }

    fn take(&mut self, square: Square) -> Option<Piece> {
        self.squares[square.index() as usize].take()
    }

    fn move_rook(&mut self, rank: u8, from_file: u8, to_file: u8) {
        let rook = self.take(Square::from_index(rank * 8 + from_file));
        self.squares[(rank * 8 + to_file) as usize] = rook;
    }

    /// No sequence of moves can mate: only kings and a single minor piece,
    /// or only bishops, all on squares of one color.
    pub(crate) fn insufficient_material(&self) -> bool {
        let mut minors = 0;
        let mut knights = 0;
        let mut bishop_square_colors = [false; 2];
        for square in Square::all() {
            match self.piece_at(square).map(|piece| piece.kind) {
                None | Some(PieceKind::King) => {}
                Some(PieceKind::Knight) => {
                    minors += 1;
                    knights += 1;
                }
                Some(PieceKind::Bishop) => {
                    minors += 1;
                    bishop_square_colors[((square.file() + square.rank()) % 2) as usize] = true;
                }
                _ => return false,
            }
        }
        minors <= 1 || (knights == 0 && !(bishop_square_colors[0] && bishop_square_colors[1]))
    }
}

/// Castling rights that end when a piece moves from or to `square`: the
/// king's and rooks' home squares.
const fn castling_lost(square: Square) -> u8 {
    match square.index() {
        0 => CastlingRights::WHITE_QUEENSIDE,
        4 => CastlingRights::WHITE_KINGSIDE | CastlingRights::WHITE_QUEENSIDE,
        7 => CastlingRights::WHITE_KINGSIDE,
        56 => CastlingRights::BLACK_QUEENSIDE,
        60 => CastlingRights::BLACK_KINGSIDE | CastlingRights::BLACK_QUEENSIDE,
        63 => CastlingRights::BLACK_KINGSIDE,
        _ => 0,
    }
}

/// Counts the leaves of the legal move tree `depth` plies deep. Comparing
/// against published totals is the standard check of a move generator.
pub fn perft(board: &Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = board.legal_moves();
    if depth == 1 {
        return moves.len() as u64;
    }
    moves
        .iter()
        .map(|&m| perft(&board.play(m), depth - 1))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::STARTPOS_FEN;
    use std::string::{String, ToString};

    // Positions and totals from https://www.chessprogramming.org/Perft_Results.
    const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    const POSITION_3: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
    const POSITION_4: &str = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
    const POSITION_5: &str = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";
    const POSITION_6: &str =
        "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10";

    fn check_perft(fen: &str, totals: &[u64]) {
        let board = Board::from_fen(fen).unwrap();
        for (depth, &nodes) in (1..).zip(totals) {
            assert_eq!(perft(&board, depth), nodes, "{fen} at depth {depth}");
        }
    }

    fn play_uci(fen: &str, uci: &str) -> String {
        let board = Board::from_fen(fen).unwrap();
        let m = board
            .parse_uci(uci)
            .unwrap_or_else(|| panic!("{uci} is not legal in {fen}"));
        board.play(m).to_string()
    }

    #[test]
    fn perft_startpos() {
        check_perft(STARTPOS_FEN, &[20, 400, 8902, 197281]);
    }

    #[test]
    fn perft_kiwipete() {
        check_perft(KIWIPETE, &[48, 2039, 97862]);
    }

    #[test]
    fn perft_position_3() {
        check_perft(POSITION_3, &[14, 191, 2812, 43238]);
    }

    #[test]
    fn perft_position_4() {
        check_perft(POSITION_4, &[6, 264, 9467]);
    }

    #[test]
    fn perft_position_5() {
        check_perft(POSITION_5, &[44, 1486, 62379]);
    }

    #[test]
    fn perft_position_6() {
        check_perft(POSITION_6, &[46, 2079, 89890]);
    }

    /// Millions of nodes each: `cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn perft_deep() {
        check_perft(STARTPOS_FEN, &[20, 400, 8902, 197281, 4865609]);
        check_perft(KIWIPETE, &[48, 2039, 97862, 4085603]);
        check_perft(POSITION_3, &[14, 191, 2812, 43238, 674624, 11030083]);
        check_perft(POSITION_4, &[6, 264, 9467, 422333]);
        check_perft(POSITION_5, &[44, 1486, 62379, 2103487]);
        check_perft(POSITION_6, &[46, 2079, 89890, 3894594]);
    }

    #[test]
    fn moves_round_trip_through_bits_and_uci() {
        for fen in [KIWIPETE, POSITION_5] {
            let board = Board::from_fen(fen).unwrap();
            for &m in board.legal_moves().iter() {
                assert_eq!(Move::from_bits(m.bits()), m);
                assert_eq!(board.parse_uci(&m.to_string()), Some(m));
            }
        }
        let promotion = Board::from_fen(POSITION_5)
            .unwrap()
            .parse_uci("d7c8n")
            .unwrap();
        assert_eq!(promotion.to_string(), "d7c8n");
        assert_eq!(promotion.kind(), MoveKind::Promotion(PieceKind::Knight));
    }

    #[test]
    fn parse_uci_rejects_illegal_and_malformed_moves() {
        let board = Board::startpos();
        for uci in ["e2e5", "e1e2", "e2e4q", "e7e5", "e2", "", "e2e4qq", "é2e4"] {
            assert_eq!(board.parse_uci(uci), None, "{uci}");
        }
        let promoting = Board::from_fen("8/4P3/8/8/8/8/k7/4K3 w - - 0 1").unwrap();
        assert_eq!(promoting.parse_uci("e7e8"), None);
        assert_eq!(promoting.parse_uci("e7e8K"), None);
        assert!(promoting.parse_uci("e7e8q").is_some());
    }

    #[test]
    fn play_updates_the_whole_position() {
        assert_eq!(
            play_uci(STARTPOS_FEN, "e2e4"),
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1"
        );
        assert_eq!(
            play_uci(STARTPOS_FEN, "g1f3"),
            "rnbqkbnr/pppppppp/8/8/8/5N2/PPPPPPPP/RNBQKB1R b KQkq - 1 1"
        );
        assert_eq!(
            play_uci("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", "e1g1"),
            "r3k2r/8/8/8/8/8/8/R4RK1 b kq - 1 1"
        );
        assert_eq!(
            play_uci("r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1", "e8c8"),
            "2kr3r/8/8/8/8/8/8/R3K2R w KQ - 1 2"
        );
        assert_eq!(
            play_uci(
                "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
                "e5d6"
            ),
            "rnbqkbnr/ppp1pppp/3P4/8/8/8/PPPP1PPP/RNBQKBNR b KQkq - 0 3"
        );
        // Capturing a rook on its home square takes away that castling right.
        assert_eq!(
            play_uci("r3k2r/8/8/8/8/8/6p1/R3K2R b KQkq - 0 1", "g2h1q"),
            "r3k2r/8/8/8/8/8/8/R3K2q w Qkq - 0 2"
        );
    }

    #[test]
    fn castling_is_blocked_by_attacks() {
        // The black bishop on a6 covers f1, so White cannot castle kingside.
        let board = Board::from_fen("4k3/8/b7/8/8/8/8/R3K2R w KQ - 0 1").unwrap();
        assert_eq!(board.parse_uci("e1g1"), None);
        assert!(board.parse_uci("e1c1").is_some());
    }

    #[test]
    fn detects_outcomes() {
        let outcome = |fen: &str| Board::from_fen(fen).unwrap().outcome();
        assert_eq!(outcome(STARTPOS_FEN), None);
        assert_eq!(
            outcome("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3"),
            Some(Outcome::Checkmate {
                winner: Color::Black
            })
        );
        assert_eq!(
            outcome("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"),
            Some(Outcome::Stalemate)
        );
        assert_eq!(
            outcome("4k3/8/8/8/8/8/8/R3K3 w - - 100 80"),
            Some(Outcome::FiftyMoveRule)
        );
        assert_eq!(
            outcome("8/8/4k3/8/8/2B5/4K3/8 w - - 0 1"),
            Some(Outcome::InsufficientMaterial)
        );
        assert_eq!(
            outcome("8/8/4k3/4b3/8/2B5/4K3/8 w - - 0 1"),
            Some(Outcome::InsufficientMaterial)
        );
        assert_eq!(outcome("8/8/4k3/3b4/8/2B5/4K3/8 w - - 0 1"), None);
        assert_eq!(outcome("8/8/4k3/8/8/2N5/4K1N1/8 w - - 0 1"), None);
    }
}
