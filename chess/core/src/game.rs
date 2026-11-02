//! A game: the start position, the moves played since and the position now.

use crate::board::{Board, CastlingRights, Color, Piece, Square};
use crate::movegen::{Move, MoveKind, Outcome};

/// Plies a game can hold. The longest decisive tournament game ran 538.
pub const MAX_PLIES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayError {
    /// The move is not legal in the current position.
    Illegal,
    /// The game already holds `MAX_PLIES` moves.
    Full,
}

/// Fixed size and no heap, so the firmware can embed it.
#[derive(Clone)]
pub struct Game {
    start: Board,
    board: Board,
    moves: [Move; MAX_PLIES],
    len: u16,
}

impl Game {
    pub fn new(start: Board) -> Game {
        Game {
            board: start.clone(),
            start,
            moves: [Move::from_bits(0); MAX_PLIES],
            len: 0,
        }
    }

    pub fn start(&self) -> &Board {
        &self.start
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn moves(&self) -> &[Move] {
        &self.moves[..self.len as usize]
    }

    pub fn last_move(&self) -> Option<Move> {
        self.moves().last().copied()
    }

    pub fn play(&mut self, m: Move) -> Result<(), PlayError> {
        if !self.board.legal_moves().contains(&m) {
            return Err(PlayError::Illegal);
        }
        if self.len as usize == MAX_PLIES {
            return Err(PlayError::Full);
        }
        self.board = self.board.play(m);
        self.moves[self.len as usize] = m;
        self.len += 1;
        Ok(())
    }

    /// Takes back the last move and returns it. Replays the game from the
    /// start, so it costs one move per ply played.
    pub fn undo(&mut self) -> Option<Move> {
        let last = self.last_move()?;
        self.len -= 1;
        self.board = self
            .moves()
            .iter()
            .fold(self.start.clone(), |board, &m| board.play(m));
        Some(last)
    }

    /// Like [`Board::outcome`], plus a draw when the current position has
    /// occurred three times or the game is full.
    pub fn outcome(&self) -> Option<Outcome> {
        self.board
            .outcome()
            .or_else(|| (self.repetitions() >= 3).then_some(Outcome::ThreefoldRepetition))
            .or_else(|| (self.len as usize == MAX_PLIES).then_some(Outcome::MoveLimit))
    }

    /// How many times the current position has occurred in this game,
    /// counting the current one.
    pub fn repetitions(&self) -> usize {
        // A capture or a pawn move resets the clock and makes every earlier
        // position unreachable, so only the last `halfmove_clock` plies count.
        let len = self.len as usize;
        let first = len - usize::from(self.board.halfmove_clock).min(len);
        let target = Position::of(&self.board);
        let mut board = self.start.clone();
        let mut count = 1;
        for (ply, &m) in self.moves().iter().enumerate() {
            if ply >= first && (len - ply) % 2 == 0 && Position::of(&board) == target {
                count += 1;
            }
            board = board.play(m);
        }
        count
    }
}

/// What makes two positions "the same" for repetition: the pieces, the side
/// to move, castling rights, and en passant only when a capture can use it.
#[derive(PartialEq, Eq)]
struct Position<'a> {
    squares: &'a [Option<Piece>; 64],
    side_to_move: Color,
    castling: CastlingRights,
    en_passant: Option<Square>,
}

impl<'a> Position<'a> {
    fn of(board: &'a Board) -> Position<'a> {
        let en_passant = board.en_passant.filter(|_| {
            board
                .legal_moves()
                .iter()
                .any(|m| m.kind() == MoveKind::EnPassant)
        });
        Position {
            squares: &board.squares,
            side_to_move: board.side_to_move,
            castling: board.castling,
            en_passant,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::PieceKind;

    #[test]
    fn a_full_game_ends_by_move_limit() {
        let mut game =
            Game::new(Board::from_fen("r3k3/pppppppp/8/8/8/8/PPPPPPPP/R3K3 w - - 0 1").unwrap());
        while game.moves().len() < MAX_PLIES {
            let board = game.board();
            let moves = board.legal_moves();
            let kind = |m: &Move| board.piece_at(m.from()).map(|piece| piece.kind);
            // The rooks shuffle on the back rank. A pawn push every 40 plies
            // keeps the 50-move rule away, and the last one makes the final
            // position new, so it is no repetition.
            let push = moves
                .iter()
                .copied()
                .find(|m| kind(m) == Some(PieceKind::Pawn) && m.from().file() == m.to().file());
            let shuffle = moves.iter().copied().find(|m| {
                kind(m) == Some(PieceKind::Rook)
                    && m.to().file() <= 1
                    && m.to().rank() == m.from().rank()
            });
            let pawn_due = board.halfmove_clock >= 40 || game.moves().len() == MAX_PLIES - 1;
            let m = if pawn_due { push } else { shuffle }.unwrap();
            game.play(m).unwrap();
        }
        assert_eq!(game.outcome(), Some(Outcome::MoveLimit));
        let any = game.board().legal_moves()[0];
        assert_eq!(game.play(any), Err(PlayError::Full));
    }

    fn play_all(game: &mut Game, moves: &[&str]) {
        for uci in moves {
            let m = game
                .board()
                .parse_uci(uci)
                .unwrap_or_else(|| panic!("{uci} is not legal"));
            game.play(m).unwrap();
        }
    }

    #[test]
    fn plays_and_takes_back() {
        let mut game = Game::new(Board::startpos());
        play_all(&mut game, &["e2e4", "e7e5", "g1f3"]);
        assert_eq!(game.moves().len(), 3);
        assert_eq!(game.last_move().map(|m| m.to()), Square::parse("f3"));

        assert_eq!(game.undo().map(|m| m.from()), Square::parse("g1"));
        assert_eq!(
            game.board(),
            &Board::from_fen("rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2")
                .unwrap()
        );
        game.undo();
        game.undo();
        assert_eq!(game.board(), &Board::startpos());
        assert_eq!(game.undo(), None);
    }

    #[test]
    fn rejects_illegal_moves() {
        let mut game = Game::new(Board::startpos());
        let black_move =
            Board::from_fen("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1")
                .unwrap()
                .parse_uci("e7e5")
                .unwrap();
        assert_eq!(game.play(black_move), Err(PlayError::Illegal));
        assert!(game.moves().is_empty());
    }

    #[test]
    fn detects_threefold_repetition() {
        let mut game = Game::new(Board::startpos());
        let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
        play_all(&mut game, &shuffle);
        assert_eq!(game.repetitions(), 2);
        assert_eq!(game.outcome(), None);
        play_all(&mut game, &shuffle);
        assert_eq!(game.repetitions(), 3);
        assert_eq!(game.outcome(), Some(Outcome::ThreefoldRepetition));
        game.undo();
        assert_eq!(game.outcome(), None);
    }

    #[test]
    fn repetition_ignores_an_unusable_en_passant_square() {
        // After 1.e4 the e3 square is set but no black pawn can take on it, so
        // the start of the shuffle already counts as the first occurrence.
        let mut game = Game::new(Board::startpos());
        play_all(&mut game, &["e2e4"]);
        let shuffle = ["g8f6", "g1f3", "f6g8", "f3g1"];
        play_all(&mut game, &shuffle);
        play_all(&mut game, &shuffle);
        assert_eq!(game.outcome(), Some(Outcome::ThreefoldRepetition));
    }

    #[test]
    fn castling_rights_make_positions_differ() {
        let mut game = Game::new(Board::from_fen("4k3/8/8/8/8/8/8/4K2R w K - 0 1").unwrap());
        // The rook leaves and returns: same squares, but castling is gone.
        play_all(
            &mut game,
            &[
                "h1h2", "e8d8", "h2h1", "d8e8", "h1h2", "e8d8", "h2h1", "d8e8",
            ],
        );
        assert_eq!(game.repetitions(), 2);
    }
}
