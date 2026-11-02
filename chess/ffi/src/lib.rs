//! C API of the CrossChess core. Keep in sync with `include/crosschess.h`.
#![cfg_attr(target_os = "none", no_std)]

use core::ffi::{CStr, c_char, c_void};
use core::fmt::{self, Write};
use core::mem::{align_of, offset_of, size_of};

use crosschess_core::{Board, Engine, Game, Limits, Move, Outcome, Piece, Square};

// Sizes and codes the header defines too; a test checks they match.
const GAME_WORDS: usize = 600;
const ENGINE_WORDS: usize = 2400;
const ONGOING: u8 = 0;
const CHECKMATE: u8 = 1;
const STALEMATE: u8 = 2;
const FIFTY_MOVES: u8 = 3;
const INSUFFICIENT_MATERIAL: u8 = 4;
const THREEFOLD_REPETITION: u8 = 5;
const MOVE_LIMIT: u8 = 6;

const VERSION: &CStr =
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(version) => version,
        Err(_) => panic!("version contains NUL"),
    };

const STARTPOS_FEN: &CStr = c"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Storage for a [`Game`] that C can embed: `cc_game` in the header.
#[repr(C)]
pub struct CcGame {
    opaque: [u32; GAME_WORDS],
}

const _: () = assert!(
    size_of::<Game>() <= size_of::<CcGame>(),
    "raise CC_GAME_WORDS in crosschess.h"
);
const _: () = assert!(align_of::<Game>() <= align_of::<CcGame>());
// C copies a game by assignment and frees one without telling Rust.
const _: () = assert!(!core::mem::needs_drop::<Game>());

/// Storage for an [`Engine`]: `cc_engine` in the header.
#[repr(C)]
pub struct CcEngine {
    opaque: [u32; ENGINE_WORDS],
}

const _: () = assert!(
    size_of::<Engine>() <= size_of::<CcEngine>(),
    "raise CC_ENGINE_WORDS in crosschess.h"
);
const _: () = assert!(align_of::<Engine>() <= align_of::<CcEngine>());
const _: () = assert!(!core::mem::needs_drop::<Engine>());

/// `cc_search_limits` in the header.
#[repr(C)]
pub struct CcSearchLimits {
    max_depth: u8,
    noise: u16,
    seed: u32,
    should_stop: Option<unsafe extern "C" fn(*mut c_void, u32) -> bool>,
    context: *mut c_void,
}

/// `cc_search_result` in the header.
#[repr(C)]
pub struct CcSearchResult {
    best: u16,
    depth: u8,
    score: i32,
    nodes: u32,
}

/// `cc_status` in the header.
#[repr(C)]
pub struct CcStatus {
    plies: u16,
    fullmove_number: u16,
    white_to_move: bool,
    in_check: bool,
    outcome: u8,
}

// The struct layouts crosschess.h asserts too, field by field.
const _: () = assert!(size_of::<CcStatus>() == 8 && offset_of!(CcStatus, outcome) == 6);
const _: () = assert!(size_of::<CcSearchResult>() == 12);
const _: () =
    assert!(offset_of!(CcSearchResult, score) == 4 && offset_of!(CcSearchResult, nodes) == 8);
const _: () =
    assert!(offset_of!(CcSearchLimits, seed) == 4 && offset_of!(CcSearchLimits, should_stop) == 8);
const _: () = assert!(offset_of!(CcSearchLimits, context) == 8 + size_of::<usize>());

/// # Safety
/// `game` must point to a `cc_game` that `cc_game_init` has filled.
unsafe fn game_ref<'a>(game: *const CcGame) -> &'a Game {
    unsafe { &*game.cast::<Game>() }
}

/// # Safety
/// As for [`game_ref`].
unsafe fn game_mut<'a>(game: *mut CcGame) -> &'a mut Game {
    unsafe { &mut *game.cast::<Game>() }
}

/// # Safety
/// `s` must be NULL or NUL-terminated.
unsafe fn str_arg<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s) }.to_str().ok()
}

/// Writes `value` and a NUL into `out[..size]`. Returns the length without
/// the NUL, or 0 (and an empty string, if there is room) when it does not fit.
///
/// # Safety
/// `out` must be NULL or point to `size` writable bytes.
unsafe fn write_c_string(value: impl fmt::Display, out: *mut c_char, size: usize) -> usize {
    struct Buffer<'a> {
        bytes: &'a mut [u8],
        len: usize,
    }
    impl Write for Buffer<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            // Keep one byte for the NUL.
            let end = self.len + s.len();
            if end >= self.bytes.len() {
                return Err(fmt::Error);
            }
            self.bytes[self.len..end].copy_from_slice(s.as_bytes());
            self.len = end;
            Ok(())
        }
    }

    if out.is_null() || size == 0 {
        return 0;
    }
    let mut buffer = Buffer {
        bytes: unsafe { core::slice::from_raw_parts_mut(out.cast::<u8>(), size) },
        len: 0,
    };
    let len = if write!(buffer, "{value}").is_ok() {
        buffer.len
    } else {
        0
    };
    buffer.bytes[len] = 0;
    len
}

fn outcome_code(outcome: Option<Outcome>) -> u8 {
    match outcome {
        None => ONGOING,
        Some(Outcome::Checkmate { .. }) => CHECKMATE,
        Some(Outcome::Stalemate) => STALEMATE,
        Some(Outcome::FiftyMoveRule) => FIFTY_MOVES,
        Some(Outcome::InsufficientMaterial) => INSUFFICIENT_MATERIAL,
        Some(Outcome::ThreefoldRepetition) => THREEFOLD_REPETITION,
        Some(Outcome::MoveLimit) => MOVE_LIMIT,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cc_version() -> *const c_char {
    VERSION.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn cc_startpos_fen() -> *const c_char {
    STARTPOS_FEN.as_ptr()
}

/// Parses `fen` and writes the piece code of every square (a1 = 0) to `out`.
/// Returns false and leaves `out` untouched if `fen` is not valid.
///
/// # Safety
/// `fen` must be NUL-terminated and `out` must point to 64 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_fen_to_squares(fen: *const c_char, out: *mut u8) -> bool {
    let Some(board) = unsafe { str_arg(fen) }.and_then(|fen| Board::from_fen(fen).ok()) else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(out, 64) };
    for (slot, square) in out.iter_mut().zip(Square::all()) {
        *slot = board.piece_at(square).map_or(0, Piece::code);
    }
    true
}

/// Starts a game from `fen`, or from the standard position when `fen` is NULL.
/// Returns false and leaves `game` untouched if `fen` is not valid.
///
/// # Safety
/// `game` must point to a writable `cc_game`; `fen` must be NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_init(game: *mut CcGame, fen: *const c_char) -> bool {
    let board = if fen.is_null() {
        Board::startpos()
    } else {
        match unsafe { str_arg(fen) }.map(Board::from_fen) {
            Some(Ok(board)) => board,
            _ => return false,
        }
    };
    if game.is_null() {
        return false;
    }
    unsafe { game.cast::<Game>().write(Game::new(board)) };
    true
}

/// # Safety
/// `game` must be initialized; `out` must point to 64 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_squares(game: *const CcGame, out: *mut u8) {
    let board = unsafe { game_ref(game) }.board();
    let out = unsafe { core::slice::from_raw_parts_mut(out, 64) };
    for (slot, square) in out.iter_mut().zip(Square::all()) {
        *slot = board.piece_at(square).map_or(0, Piece::code);
    }
}

/// # Safety
/// `game` must be initialized; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_status(game: *const CcGame, out: *mut CcStatus) {
    let game = unsafe { game_ref(game) };
    let board = game.board();
    let status = CcStatus {
        plies: game.moves().len() as u16,
        fullmove_number: board.fullmove_number(),
        white_to_move: board.side_to_move() == crosschess_core::Color::White,
        in_check: board.in_check(),
        outcome: outcome_code(game.outcome()),
    };
    unsafe { out.write(status) };
}

/// Writes the legal moves and returns how many there are.
///
/// # Safety
/// `game` must be initialized; `out` must point to `CC_MAX_MOVES` writable `uint16_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_legal_moves(game: *const CcGame, out: *mut u16) -> usize {
    let moves = unsafe { game_ref(game) }.board().legal_moves();
    let out = unsafe { core::slice::from_raw_parts_mut(out, crosschess_core::MAX_MOVES) };
    for (slot, m) in out.iter_mut().zip(moves.iter()) {
        *slot = m.bits();
    }
    moves.len()
}

/// Plays `move` if it is legal. Returns false and changes nothing otherwise.
///
/// # Safety
/// `game` must be initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_play(game: *mut CcGame, m: u16) -> bool {
    unsafe { game_mut(game) }.play(Move::from_bits(m)).is_ok()
}

/// Takes back the last move. Returns false at the start of the game.
///
/// # Safety
/// `game` must be initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_undo(game: *mut CcGame) -> bool {
    unsafe { game_mut(game) }.undo().is_some()
}

/// The move played at `ply` (0 is the first), or 0 past the end.
///
/// # Safety
/// `game` must be initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_move(game: *const CcGame, ply: usize) -> u16 {
    unsafe { game_ref(game) }
        .moves()
        .get(ply)
        .map_or(0, |m| m.bits())
}

/// The legal move written in UCI ("e2e4", "e7e8q"), or 0.
///
/// # Safety
/// `game` must be initialized; `uci` must be NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_parse_uci(game: *const CcGame, uci: *const c_char) -> u16 {
    let board = unsafe { game_ref(game) }.board();
    unsafe { str_arg(uci) }
        .and_then(|uci| board.parse_uci(uci))
        .map_or(0, |m| m.bits())
}

/// SAN of `move` in the current position, which must make it legal.
/// Returns the length, or 0 if the move is not legal or `out` is too small.
///
/// # Safety
/// `game` must be initialized; `out` must point to `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_san(
    game: *const CcGame,
    m: u16,
    out: *mut c_char,
    size: usize,
) -> usize {
    let board = unsafe { game_ref(game) }.board();
    let m = Move::from_bits(m);
    if !board.legal_moves().contains(&m) {
        return unsafe { write_c_string("", out, size) };
    }
    unsafe { write_c_string(board.san(m), out, size) }
}

/// FEN of the start position (`start`) or of the current one.
/// Returns the length, or 0 if `out` is too small.
///
/// # Safety
/// `game` must be initialized; `out` must point to `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_game_fen(
    game: *const CcGame,
    start: bool,
    out: *mut c_char,
    size: usize,
) -> usize {
    let game = unsafe { game_ref(game) };
    let board = if start { game.start() } else { game.board() };
    unsafe { write_c_string(board, out, size) }
}

/// UCI of `move`. Returns the length (4 or 5).
///
/// # Safety
/// `out` must point to 6 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_move_uci(m: u16, out: *mut c_char) -> usize {
    unsafe { write_c_string(Move::from_bits(m), out, 6) }
}

/// Searches the current position of `game`. Returns false, with `best` 0,
/// when the side to move has no legal move.
///
/// # Safety
/// `engine` must point to a `cc_engine` that is zero-filled or was used
/// before; `game` must be initialized; `limits` must be valid and its
/// callback safe to call with its context; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cc_engine_search(
    engine: *mut CcEngine,
    game: *const CcGame,
    limits: *const CcSearchLimits,
    out: *mut CcSearchResult,
) -> bool {
    let engine = unsafe { &mut *engine.cast::<Engine>() };
    let game = unsafe { game_ref(game) };
    let limits = unsafe { &*limits };
    let mut control = |nodes: u32| match limits.should_stop {
        Some(should_stop) => unsafe { should_stop(limits.context, nodes) },
        None => false,
    };
    let result = engine.search(
        game,
        &Limits {
            max_depth: limits.max_depth,
            noise: limits.noise,
            seed: limits.seed,
        },
        &mut control,
    );
    let summary = CcSearchResult {
        best: result.best.map_or(0, |m| m.bits()),
        depth: result.depth,
        score: result.score,
        nodes: result.nodes,
    };
    unsafe { out.write(summary) };
    result.best.is_some()
}

// ESP-IDF's abort() logs the reason and resets the chip.
#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn abort() -> !;
    }
    unsafe { abort() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;
    use std::mem::MaybeUninit;

    fn new_game(fen: Option<&CStr>) -> Box<CcGame> {
        let mut game = Box::new(MaybeUninit::<CcGame>::uninit());
        assert!(unsafe {
            cc_game_init(
                game.as_mut_ptr(),
                fen.map_or(core::ptr::null(), CStr::as_ptr),
            )
        });
        unsafe { game.assume_init() }
    }

    fn status(game: &CcGame) -> CcStatus {
        let mut status = MaybeUninit::uninit();
        unsafe { cc_game_status(game, status.as_mut_ptr()) };
        unsafe { status.assume_init() }
    }

    fn string(buffer: &[u8]) -> &str {
        CStr::from_bytes_until_nul(buffer)
            .unwrap()
            .to_str()
            .unwrap()
    }

    fn play(game: &mut CcGame, uci: &CStr) {
        let m = unsafe { cc_game_parse_uci(game, uci.as_ptr()) };
        assert_ne!(m, 0, "{uci:?}");
        assert!(unsafe { cc_game_play(game, m) });
    }

    /// The `CC_*` numbers of the header: enum members and `#define`s.
    fn header_constants() -> std::collections::HashMap<&'static str, i64> {
        let mut constants = std::collections::HashMap::new();
        for line in include_str!("../../include/crosschess.h").lines() {
            let code = line.split("//").next().unwrap().trim();
            let pair = match code.strip_prefix("#define ") {
                Some(define) => define.split_once(' '),
                None => code
                    .strip_suffix(',')
                    .and_then(|member| member.split_once(" = ")),
            };
            let Some((name, value)) = pair.filter(|(name, _)| name.starts_with("CC_")) else {
                continue;
            };
            if let Ok(value) = value.trim().parse() {
                constants.insert(name, value);
            }
        }
        constants
    }

    #[test]
    fn header_matches_rust() {
        use crosschess_core::{Color, PieceKind};
        let header = header_constants();
        let check = |name: &str, value: i64| {
            assert_eq!(header.get(name), Some(&value), "{name} in crosschess.h")
        };

        check("CC_GAME_WORDS", GAME_WORDS as i64);
        check("CC_ENGINE_WORDS", ENGINE_WORDS as i64);
        check("CC_MAX_MOVES", crosschess_core::MAX_MOVES as i64);
        check("CC_MAX_PLIES", crosschess_core::MAX_PLIES as i64);
        check("CC_MATE", crosschess_core::MATE as i64);
        for (name, code) in [
            ("CC_ONGOING", ONGOING),
            ("CC_CHECKMATE", CHECKMATE),
            ("CC_STALEMATE", STALEMATE),
            ("CC_FIFTY_MOVES", FIFTY_MOVES),
            ("CC_INSUFFICIENT_MATERIAL", INSUFFICIENT_MATERIAL),
            ("CC_THREEFOLD_REPETITION", THREEFOLD_REPETITION),
            ("CC_MOVE_LIMIT", MOVE_LIMIT),
        ] {
            check(name, code.into());
        }

        let pieces = [
            ("CC_PAWN", PieceKind::Pawn),
            ("CC_KNIGHT", PieceKind::Knight),
            ("CC_BISHOP", PieceKind::Bishop),
            ("CC_ROOK", PieceKind::Rook),
            ("CC_QUEEN", PieceKind::Queen),
            ("CC_KING", PieceKind::King),
        ];
        for (name, kind) in pieces {
            check(name, Piece::new(Color::White, kind).code().into());
            let black =
                Piece::new(Color::Black, kind).code() - Piece::new(Color::White, kind).code();
            check("CC_BLACK", black.into());
        }

        // Move kinds as real moves encode them; cc_move_kind() is bits 12-15.
        for (fen, uci, name) in [
            (crosschess_core::STARTPOS_FEN, "g1f3", "CC_MOVE_NORMAL"),
            (crosschess_core::STARTPOS_FEN, "e2e4", "CC_MOVE_DOUBLE_PUSH"),
            (
                "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1",
                "e1g1",
                "CC_MOVE_CASTLE_KINGSIDE",
            ),
            (
                "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1",
                "e1c1",
                "CC_MOVE_CASTLE_QUEENSIDE",
            ),
            (
                "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1",
                "e5d6",
                "CC_MOVE_EN_PASSANT",
            ),
            (
                "4k3/P7/8/8/8/8/8/4K3 w - - 0 1",
                "a7a8n",
                "CC_MOVE_PROMOTE_KNIGHT",
            ),
            (
                "4k3/P7/8/8/8/8/8/4K3 w - - 0 1",
                "a7a8b",
                "CC_MOVE_PROMOTE_BISHOP",
            ),
            (
                "4k3/P7/8/8/8/8/8/4K3 w - - 0 1",
                "a7a8r",
                "CC_MOVE_PROMOTE_ROOK",
            ),
            (
                "4k3/P7/8/8/8/8/8/4K3 w - - 0 1",
                "a7a8q",
                "CC_MOVE_PROMOTE_QUEEN",
            ),
        ] {
            let m = Board::from_fen(fen).unwrap().parse_uci(uci).unwrap();
            check(name, (m.bits() >> 12).into());
        }
    }

    #[test]
    fn startpos_matches_core() {
        assert_eq!(
            STARTPOS_FEN.to_str().unwrap(),
            crosschess_core::STARTPOS_FEN
        );
    }

    #[test]
    fn fen_to_squares_fills_board() {
        let mut squares = [0xFFu8; 64];
        assert!(unsafe { cc_fen_to_squares(cc_startpos_fen(), squares.as_mut_ptr()) });
        assert_eq!(squares[4], 6); // e1: white king
        assert_eq!(squares[60], 14); // e8: black king
        assert_eq!(squares[28], 0); // e4: empty
    }

    #[test]
    fn fen_to_squares_rejects_bad_input() {
        let mut squares = [0xFFu8; 64];
        assert!(!unsafe { cc_fen_to_squares(c"not a fen".as_ptr(), squares.as_mut_ptr()) });
        assert!(!unsafe { cc_fen_to_squares(core::ptr::null(), squares.as_mut_ptr()) });
        assert_eq!(squares, [0xFF; 64]);
    }

    #[test]
    fn game_init_rejects_bad_fen() {
        let mut game = MaybeUninit::<CcGame>::uninit();
        assert!(!unsafe { cc_game_init(game.as_mut_ptr(), c"8/8 w".as_ptr()) });
    }

    #[test]
    fn plays_a_game_through_the_c_api() {
        let mut game = new_game(None);
        let mut moves = [0u16; crosschess_core::MAX_MOVES];
        assert_eq!(
            unsafe { cc_game_legal_moves(&*game, moves.as_mut_ptr()) },
            20
        );

        // A move from the other side, or garbage bits, is refused.
        assert!(!unsafe { cc_game_play(&mut *game, 0) });
        assert_eq!(unsafe { cc_game_parse_uci(&*game, c"e7e5".as_ptr()) }, 0);

        let mut san = [0u8; 8];
        for (uci, expected) in [
            (c"f2f3", "f3"),
            (c"e7e5", "e5"),
            (c"g2g4", "g4"),
            (c"d8h4", "Qh4#"),
        ] {
            let m = unsafe { cc_game_parse_uci(&*game, uci.as_ptr()) };
            assert!(unsafe { cc_game_san(&*game, m, san.as_mut_ptr().cast(), san.len()) } > 0);
            assert_eq!(string(&san), expected);
            play(&mut game, uci);
        }

        let s = status(&game);
        assert_eq!(
            (
                s.plies,
                s.fullmove_number,
                s.white_to_move,
                s.in_check,
                s.outcome
            ),
            (4, 3, true, true, 1)
        );

        let mut uci = [0u8; 6];
        assert_eq!(
            unsafe { cc_move_uci(cc_game_move(&*game, 3), uci.as_mut_ptr().cast()) },
            4
        );
        assert_eq!(string(&uci), "d8h4");
        assert_eq!(unsafe { cc_game_move(&*game, 4) }, 0);

        assert!(unsafe { cc_game_undo(&mut *game) });
        let s = status(&game);
        assert_eq!(
            (s.plies, s.white_to_move, s.in_check, s.outcome),
            (3, false, false, 0)
        );
    }

    unsafe extern "C" fn count_calls(context: *mut c_void, _nodes: u32) -> bool {
        unsafe { *context.cast::<u32>() += 1 };
        false
    }

    #[test]
    fn engine_searches_through_the_c_api() {
        let game = new_game(Some(c"6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1"));
        // Zero-filled storage, as the firmware allocates it.
        let mut engine = Box::new(MaybeUninit::<CcEngine>::zeroed());
        let mut calls = 0u32;
        let limits = CcSearchLimits {
            max_depth: 3,
            noise: 0,
            seed: 0,
            should_stop: Some(count_calls),
            context: (&raw mut calls).cast(),
        };
        let mut result = MaybeUninit::<CcSearchResult>::uninit();
        assert!(unsafe {
            cc_engine_search(engine.as_mut_ptr(), &*game, &limits, result.as_mut_ptr())
        });
        let result = unsafe { result.assume_init() };
        let mut uci = [0u8; 6];
        unsafe { cc_move_uci(result.best, uci.as_mut_ptr().cast()) };
        assert_eq!(string(&uci), "a1a8");
        assert_eq!(result.score, crosschess_core::MATE - 1);
        assert_eq!(calls, result.nodes / 1024);

        let stalemate = new_game(Some(c"7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"));
        let mut result = MaybeUninit::<CcSearchResult>::uninit();
        let limits = CcSearchLimits {
            should_stop: None,
            ..limits
        };
        assert!(!unsafe {
            cc_engine_search(
                engine.as_mut_ptr(),
                &*stalemate,
                &limits,
                result.as_mut_ptr(),
            )
        });
        assert_eq!(unsafe { result.assume_init() }.best, 0);
    }

    #[test]
    fn fen_and_san_respect_the_buffer_size() {
        let mut game = new_game(Some(c"4k3/8/8/8/8/8/8/R3K3 w Q - 0 40"));
        let mut fen = [0xAAu8; 64];
        let len = unsafe { cc_game_fen(&*game, true, fen.as_mut_ptr().cast(), fen.len()) };
        assert_eq!(string(&fen), "4k3/8/8/8/8/8/8/R3K3 w Q - 0 40");
        assert_eq!(len, 31);

        // Exactly the length leaves no room for the NUL.
        assert_eq!(
            unsafe { cc_game_fen(&*game, true, fen.as_mut_ptr().cast(), len) },
            0
        );
        assert_eq!(fen[0], 0);

        play(&mut game, c"a1a8");
        assert_eq!(
            unsafe { cc_game_fen(&*game, false, fen.as_mut_ptr().cast(), fen.len()) },
            31
        );
        assert_eq!(string(&fen), "R3k3/8/8/8/8/8/8/4K3 b - - 1 40");

        let mut san = [0u8; 8];
        assert_eq!(
            unsafe { cc_game_san(&*game, 0, san.as_mut_ptr().cast(), san.len()) },
            0
        );
        assert_eq!(string(&san), "");
    }
}
