//! Zobrist hashing: a 64-bit key per position, so the engine can spot
//! repetitions without comparing whole boards.

use crate::board::{Board, Color, Piece, PieceKind, Square};

struct Keys {
    pieces: [[u64; 64]; 12],
    castling: [u64; 16],
    en_passant: [u64; 8],
    black_to_move: u64,
}

/// SplitMix64: a fixed, well-mixed sequence, computed at compile time.
const fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const KEYS: Keys = {
    let mut state = 0x4372_6F73_7343_6865; // "CrossChe"
    let mut keys = Keys {
        pieces: [[0; 64]; 12],
        castling: [0; 16],
        en_passant: [0; 8],
        black_to_move: 0,
    };
    let mut piece = 0;
    while piece < 12 {
        let mut square = 0;
        while square < 64 {
            keys.pieces[piece][square] = splitmix(&mut state);
            square += 1;
        }
        piece += 1;
    }
    let mut i = 0;
    while i < 16 {
        keys.castling[i] = splitmix(&mut state);
        i += 1;
    }
    i = 0;
    while i < 8 {
        keys.en_passant[i] = splitmix(&mut state);
        i += 1;
    }
    keys.black_to_move = splitmix(&mut state);
    keys
};

impl Board {
    /// Positions that are the same for repetition get the same key: the
    /// pieces, the side to move, castling rights, and the en passant square
    /// only when a pawn stands ready to use it. The clocks are left out.
    pub fn hash(&self) -> u64 {
        let mut key = 0;
        for square in Square::all() {
            if let Some(piece) = self.piece_at(square) {
                let index = (piece.kind as usize - 1) + 6 * piece.color as usize;
                key ^= KEYS.pieces[index][square.index() as usize];
            }
        }
        key ^= KEYS.castling[self.castling.bits() as usize & 15];
        if let Some(target) = self.en_passant
            && self.en_passant_usable(target)
        {
            key ^= KEYS.en_passant[target.file() as usize];
        }
        if self.side_to_move == Color::Black {
            key ^= KEYS.black_to_move;
        }
        key
    }

    /// Whether a pawn of the side to move stands next to the pawn that just
    /// made a double step. Pins are ignored; this only has to be stable.
    fn en_passant_usable(&self, target: Square) -> bool {
        let us = self.side_to_move;
        let behind = match us {
            Color::White => -1,
            Color::Black => 1,
        };
        let pawn = Some(Piece::new(us, PieceKind::Pawn));
        [-1, 1].into_iter().any(|files| {
            target
                .offset(files, behind)
                .is_some_and(|square| self.piece_at(square) == pawn)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::STARTPOS_FEN;

    fn hash_after(fen: &str, moves: &[&str]) -> u64 {
        let mut board = Board::from_fen(fen).unwrap();
        for uci in moves {
            board = board.play(board.parse_uci(uci).unwrap());
        }
        board.hash()
    }

    #[test]
    fn transpositions_share_a_key() {
        let a = hash_after(STARTPOS_FEN, &["g1f3", "g8f6", "b1c3", "b8c6"]);
        let b = hash_after(STARTPOS_FEN, &["b1c3", "b8c6", "g1f3", "g8f6"]);
        assert_eq!(a, b);
        assert_ne!(a, hash_after(STARTPOS_FEN, &["g1f3", "g8f6", "b1c3"]));
    }

    #[test]
    fn clocks_do_not_matter_but_rights_do() {
        let base = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        assert_eq!(
            Board::from_fen(base).unwrap().hash(),
            hash_after("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 17 40", &[])
        );
        assert_ne!(
            Board::from_fen(base).unwrap().hash(),
            hash_after("r3k2r/8/8/8/8/8/8/R3K2R w Kkq - 0 1", &[])
        );
        assert_ne!(
            Board::from_fen(base).unwrap().hash(),
            hash_after("r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1", &[])
        );
    }

    #[test]
    fn en_passant_counts_only_when_usable() {
        // After 1.e4 no black pawn can take on e3: same key as without the square.
        assert_eq!(
            hash_after(STARTPOS_FEN, &["e2e4"]),
            hash_after(
                "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
                &[]
            )
        );
        // A black pawn on d4 can, so the square changes the key.
        let usable = "rnbqkbnr/ppp1pppp/8/8/3pP3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 3";
        assert_ne!(
            hash_after(usable, &[]),
            hash_after(
                "rnbqkbnr/ppp1pppp/8/8/3pP3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 3",
                &[]
            )
        );
    }
}
