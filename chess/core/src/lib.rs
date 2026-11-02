#![no_std]

#[cfg(test)]
extern crate std;

pub mod board;
pub mod engine;
mod eval;
pub mod game;
mod hash;
pub mod movegen;
pub mod san;

pub use board::{Board, Color, FenError, Piece, PieceKind, STARTPOS_FEN, Square};
pub use engine::{Control, Engine, Limits, MATE, SearchResult};
pub use game::{Game, MAX_PLIES, PlayError};
pub use movegen::{MAX_MOVES, Move, MoveKind, MoveList, Outcome, perft};
pub use san::San;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
