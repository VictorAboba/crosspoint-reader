//! The engine: alpha-beta search with iterative deepening.
//!
//! - Quiescence search on captures and queen promotions, so a line does not
//!   end in the middle of an exchange.
//! - A position in check is searched one ply deeper.
//! - Move order: the previous iteration's best move at the root, then captures
//!   by most valuable victim and least valuable attacker, then killer moves.
//! - Draws by repetition (also against the game so far) and the 50-move rule.
//! - Below full strength, noise is added to the evaluation. It depends on the
//!   position and the seed, so one search sees a position the same way each
//!   time it meets it; a new seed per search varies the play.
//!
//! No heap and little stack: the caller owns an [`Engine`] of about 9 KB.

use crate::board::{Board, PieceKind};
use crate::eval::evaluate;
use crate::game::Game;
use crate::movegen::{Move, MoveKind};

/// Deepest ply the search reaches, extensions and quiescence included.
pub const MAX_PLY: usize = 32;
/// Score of a mate on the board; a mate in n plies scores `MATE - n`.
pub const MATE: i32 = 30_000;
const INFINITY: i32 = 32_000;
/// Moves of every ply on the current line share one stack.
const MOVE_STACK: usize = 2048;
/// Earlier positions of the game kept for repetition; the 50-move rule ends
/// the game before more than 100 can matter.
const HISTORY: usize = 112;
const CHECK_EVERY: u32 = 1024;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Iterations to run; the search may stop earlier through [`Control`].
    pub max_depth: u8,
    /// Evaluation noise in centipawns, 0 for the best play.
    pub noise: u16,
    pub seed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchResult {
    /// None only when the side to move has no legal move.
    pub best: Option<Move>,
    /// From the side to move's point of view, in centipawns.
    pub score: i32,
    /// Last fully searched depth.
    pub depth: u8,
    pub nodes: u32,
}

/// Asked every 1024 nodes, with the node count, whether to stop. The first
/// iteration always finishes, so there is always a move.
pub trait Control {
    fn should_stop(&mut self, nodes: u32) -> bool;
}

impl<F: FnMut(u32) -> bool> Control for F {
    fn should_stop(&mut self, nodes: u32) -> bool {
        self(nodes)
    }
}

/// Scratch space of the search. Every field is a plain integer, so zeroed
/// memory is a valid `Engine`: the firmware can allocate one without Rust.
pub struct Engine {
    moves: [Move; MOVE_STACK],
    scores: [i16; MOVE_STACK],
    /// Position keys, halves of the 64-bit hash so 4-byte alignment does:
    /// the game's recent positions, then one per ply of the current line.
    keys: [[u32; 2]; HISTORY + MAX_PLY + 1],
    killers: [[Move; 2]; MAX_PLY + 1],
    history_len: u32,
    top: u32,
    nodes: u32,
    noise: u32,
    seed: u32,
    stopped: bool,
    can_stop: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub const fn new() -> Engine {
        Engine {
            moves: [Move::from_bits(0); MOVE_STACK],
            scores: [0; MOVE_STACK],
            keys: [[0; 2]; HISTORY + MAX_PLY + 1],
            killers: [[Move::from_bits(0); 2]; MAX_PLY + 1],
            history_len: 0,
            top: 0,
            nodes: 0,
            noise: 0,
            seed: 0,
            stopped: false,
            can_stop: false,
        }
    }

    /// Finds a move for the side to move in `game`.
    pub fn search(
        &mut self,
        game: &Game,
        limits: &Limits,
        control: &mut impl Control,
    ) -> SearchResult {
        self.killers = [[Move::from_bits(0); 2]; MAX_PLY + 1];
        self.top = 0;
        self.nodes = 0;
        self.noise = limits.noise.into();
        self.seed = limits.seed;
        self.stopped = false;
        self.can_stop = false;
        self.load_history(game);

        let root = game.board();
        let mut result = SearchResult {
            best: None,
            score: 0,
            depth: 0,
            nodes: 0,
        };
        for depth in 1..=limits.max_depth.clamp(1, MAX_PLY as u8 - 1) {
            let (best, score) = self.search_root(root, depth, result.best, control);
            // An unfinished iteration searched the previous best move first,
            // so any move it found better is safe to take.
            if best.is_some() {
                result.best = best;
                result.score = score;
            }
            if self.stopped || best.is_none() {
                break;
            }
            result.depth = depth;
            self.can_stop = true;
            if score.abs() >= MATE - MAX_PLY as i32 {
                break;
            }
        }
        result.nodes = self.nodes;
        result
    }

    /// Keys of the positions since the last capture or pawn move, oldest
    /// first, ending just before the current one.
    fn load_history(&mut self, game: &Game) {
        let moves = game.moves();
        let window = usize::from(game.board().halfmove_clock)
            .min(moves.len())
            .min(HISTORY);
        let first = moves.len() - window;
        let mut board = game.start().clone();
        let mut count = 0;
        for (ply, &m) in moves.iter().enumerate() {
            if ply >= first {
                self.keys[count] = split(board.hash());
                count += 1;
            }
            board = board.play(m);
        }
        self.history_len = count as u32;
    }

    fn search_root(
        &mut self,
        board: &Board,
        depth: u8,
        previous: Option<Move>,
        control: &mut impl Control,
    ) -> (Option<Move>, i32) {
        self.keys[self.history_len as usize] = split(board.hash());
        let (start, end) = self.generate(board, false);
        self.score_moves(board, start, end, 0, previous);
        let mut best = None;
        let mut alpha = -INFINITY;
        for i in start..end {
            let m = self.pick(i, end);
            let next = board.play(m);
            if !board.is_legal_result(&next) {
                continue;
            }
            let score = -self.negamax(&next, depth - 1, 1, -INFINITY, -alpha, control);
            if self.stopped {
                break;
            }
            if best.is_none() || score > alpha {
                alpha = score;
                best = Some(m);
            }
        }
        self.top = start as u32;
        (best, alpha)
    }

    fn negamax(
        &mut self,
        board: &Board,
        depth: u8,
        ply: usize,
        mut alpha: i32,
        beta: i32,
        control: &mut impl Control,
    ) -> i32 {
        if self.tick(control) {
            return 0;
        }
        let index = self.history_len as usize + ply;
        self.keys[index] = split(board.hash());
        if board.halfmove_clock >= 100
            || board.insufficient_material()
            || self.repeated(index, board.halfmove_clock)
        {
            return 0;
        }
        let in_check = board.in_check();
        let depth = if in_check { depth + 1 } else { depth };
        if depth == 0 || ply >= MAX_PLY {
            return self.quiesce(board, ply, alpha, beta, control);
        }

        let (start, end) = self.generate(board, false);
        self.score_moves(board, start, end, ply, None);
        let mut best = -INFINITY;
        let mut legal = 0;
        for i in start..end {
            let m = self.pick(i, end);
            let next = board.play(m);
            if !board.is_legal_result(&next) {
                continue;
            }
            legal += 1;
            let score = -self.negamax(&next, depth - 1, ply + 1, -beta, -alpha, control);
            if self.stopped {
                break;
            }
            best = best.max(score);
            if score > alpha {
                alpha = score;
                if alpha >= beta {
                    if !is_tactical(board, m) {
                        self.add_killer(ply, m);
                    }
                    break;
                }
            }
        }
        self.top = start as u32;
        if self.stopped {
            return 0;
        }
        if legal == 0 {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        best
    }

    /// Searches captures and queen promotions until the position is quiet.
    /// In check every move is searched, so mates are still seen.
    fn quiesce(
        &mut self,
        board: &Board,
        ply: usize,
        mut alpha: i32,
        beta: i32,
        control: &mut impl Control,
    ) -> i32 {
        if self.tick(control) {
            return 0;
        }
        if ply >= MAX_PLY {
            return self.evaluate(board);
        }
        let in_check = board.in_check();
        let mut best = -INFINITY;
        if !in_check {
            // Standing pat: the side to move need not capture.
            best = self.evaluate(board);
            if best >= beta {
                return best;
            }
            alpha = alpha.max(best);
        }

        let (start, end) = self.generate(board, !in_check);
        self.score_moves(board, start, end, ply, None);
        let mut legal = 0;
        for i in start..end {
            let m = self.pick(i, end);
            let next = board.play(m);
            if !board.is_legal_result(&next) {
                continue;
            }
            legal += 1;
            let score = -self.quiesce(&next, ply + 1, -beta, -alpha, control);
            if self.stopped {
                break;
            }
            best = best.max(score);
            if score > alpha {
                alpha = score;
                if alpha >= beta {
                    break;
                }
            }
        }
        self.top = start as u32;
        if self.stopped {
            return 0;
        }
        if in_check && legal == 0 {
            return -MATE + ply as i32;
        }
        best
    }

    fn tick(&mut self, control: &mut impl Control) -> bool {
        self.nodes = self.nodes.wrapping_add(1);
        // Ask even before stopping is allowed: the firmware yields to other
        // tasks in its callback.
        if self.nodes % CHECK_EVERY == 0 && control.should_stop(self.nodes) && self.can_stop {
            self.stopped = true;
        }
        self.stopped
    }

    fn evaluate(&self, board: &Board) -> i32 {
        let score = evaluate(board);
        if self.noise == 0 {
            return score;
        }
        let span = u64::from(2 * self.noise + 1);
        score + (mix(board.hash() ^ u64::from(self.seed)) % span) as i32 - self.noise as i32
    }

    /// Whether the position at `index` of the key stack occurred before,
    /// within the plies since the last capture or pawn move.
    fn repeated(&self, index: usize, halfmove_clock: u16) -> bool {
        let key = self.keys[index];
        let oldest = index.saturating_sub(usize::from(halfmove_clock));
        let mut i = index;
        while i >= oldest + 2 {
            i -= 2;
            if self.keys[i] == key {
                return true;
            }
        }
        false
    }

    /// Pushes pseudo-legal moves (only tactical ones if `tactical_only`) on
    /// the move stack and returns their range.
    fn generate(&mut self, board: &Board, tactical_only: bool) -> (usize, usize) {
        let start = self.top as usize;
        let mut end = start;
        let moves = &mut self.moves;
        board.pseudo_legal_moves(&mut |m| {
            if end < MOVE_STACK && (!tactical_only || is_tactical(board, m)) {
                moves[end] = m;
                end += 1;
            }
        });
        self.top = end as u32;
        (start, end)
    }

    fn score_moves(
        &mut self,
        board: &Board,
        start: usize,
        end: usize,
        ply: usize,
        first: Option<Move>,
    ) {
        let killers = self.killers[ply];
        for i in start..end {
            let m = self.moves[i];
            self.scores[i] = if Some(m) == first {
                i16::MAX
            } else if let Some(victim) = captured(board, m) {
                let attacker = board
                    .piece_at(m.from())
                    .map_or(0, |piece| piece.kind as i16);
                10_000 + 100 * victim as i16 - attacker
            } else if m.promotion() == Some(PieceKind::Queen) {
                9_000
            } else if m == killers[0] {
                8_000
            } else if m == killers[1] {
                7_000
            } else if m.promotion().is_some() {
                -1_000
            } else {
                0
            };
        }
    }

    /// Moves the best-scored move of `i..end` to `i` and returns it.
    fn pick(&mut self, i: usize, end: usize) -> Move {
        let mut best = i;
        for j in i + 1..end {
            if self.scores[j] > self.scores[best] {
                best = j;
            }
        }
        self.moves.swap(i, best);
        self.scores.swap(i, best);
        self.moves[i]
    }

    fn add_killer(&mut self, ply: usize, m: Move) {
        let killers = &mut self.killers[ply];
        if killers[0] != m {
            killers[1] = killers[0];
            killers[0] = m;
        }
    }
}

/// The kind of piece `m` takes, if any.
fn captured(board: &Board, m: Move) -> Option<PieceKind> {
    if m.kind() == MoveKind::EnPassant {
        return Some(PieceKind::Pawn);
    }
    board.piece_at(m.to()).map(|piece| piece.kind)
}

fn is_tactical(board: &Board, m: Move) -> bool {
    captured(board, m).is_some() || m.promotion() == Some(PieceKind::Queen)
}

fn split(key: u64) -> [u32; 2] {
    [key as u32, (key >> 32) as u32]
}

/// SplitMix64's finaliser: spreads the bits of a key for the noise.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movegen::Outcome;
    use std::boxed::Box;
    use std::string::ToString;

    fn best_move(fen: &str, depth: u8) -> SearchResult {
        let game = Game::new(Board::from_fen(fen).unwrap());
        let mut engine = Box::new(Engine::new());
        engine.search(
            &game,
            &Limits {
                max_depth: depth,
                noise: 0,
                seed: 0,
            },
            &mut |_| false,
        )
    }

    fn uci(result: &SearchResult) -> std::string::String {
        result.best.map(|m| m.to_string()).unwrap_or_default()
    }

    #[test]
    fn mates_in_one() {
        let result = best_move("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1", 2);
        assert_eq!(uci(&result), "a1a8");
        assert_eq!(result.score, MATE - 1);
    }

    #[test]
    fn mates_in_two() {
        // 1.Rd8+ Rxd8 2.Rxd8#.
        let result = best_move("2r3k1/5ppp/8/8/8/8/3R1PPP/3R2K1 w - - 0 1", 4);
        assert_eq!(uci(&result), "d2d8");
        assert_eq!(result.score, MATE - 3);
    }

    #[test]
    fn takes_a_hanging_queen() {
        assert_eq!(
            uci(&best_move("4k3/8/8/3q4/8/8/3R4/4K3 w - - 0 1", 1)),
            "d2d5"
        );
    }

    #[test]
    fn mates_instead_of_stalemating() {
        // Qg6 would be stalemate; Qg7 is mate.
        assert_eq!(uci(&best_move("7k/8/5K2/8/8/8/8/6Q1 w - - 0 1", 3)), "g1g7");
    }

    #[test]
    fn reports_no_move_when_there_is_none() {
        let result = best_move("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1", 3);
        assert_eq!(result.best, None);
    }

    #[test]
    fn avoids_repeating_when_winning() {
        let mut game = Game::new(Board::from_fen("7k/8/8/8/8/8/8/1Q5K w - - 0 1").unwrap());
        for uci in ["b1c1", "h8g8", "c1b1", "g8h8"] {
            game.play(game.board().parse_uci(uci).unwrap()).unwrap();
        }
        let mut engine = Box::new(Engine::new());
        let result = engine.search(
            &game,
            &Limits {
                max_depth: 3,
                noise: 0,
                seed: 0,
            },
            &mut |_| false,
        );
        assert_ne!(uci(&result), "b1c1", "b1c1 repeats the position");
        assert!(result.score > 500);
    }

    #[test]
    fn stops_when_asked_but_finishes_the_first_iteration() {
        let game = Game::new(Board::startpos());
        let mut engine = Box::new(Engine::new());
        let result = engine.search(
            &game,
            &Limits {
                max_depth: 20,
                noise: 0,
                seed: 0,
            },
            &mut |_| true,
        );
        assert!(result.best.is_some());
        assert!(game.board().legal_moves().contains(&result.best.unwrap()));
        assert!(result.depth <= 2);
    }

    #[test]
    fn noise_changes_moves_but_keeps_them_legal() {
        let game = Game::new(Board::startpos());
        let mut engine = Box::new(Engine::new());
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..12 {
            let result = engine.search(
                &game,
                &Limits {
                    max_depth: 2,
                    noise: 80,
                    seed,
                },
                &mut |_| false,
            );
            let m = result.best.unwrap();
            assert!(game.board().legal_moves().contains(&m));
            seen.insert(m.bits());
        }
        assert!(seen.len() > 1, "noise should vary the choice");
    }

    #[test]
    fn zeroed_memory_is_a_ready_engine() {
        let mut storage = Box::new(core::mem::MaybeUninit::<Engine>::zeroed());
        let engine = unsafe { storage.assume_init_mut() };
        let game = Game::new(Board::from_fen("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1").unwrap());
        let result = engine.search(
            &game,
            &Limits {
                max_depth: 2,
                noise: 0,
                seed: 0,
            },
            &mut |_| false,
        );
        assert_eq!(uci(&result), "a1a8");
    }

    /// Prints speed per depth: `cargo test --release -- --ignored --nocapture engine_speed`.
    #[test]
    #[ignore]
    fn engine_speed() {
        for fen in [
            crate::board::STARTPOS_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        ] {
            let game = Game::new(Board::from_fen(fen).unwrap());
            let mut engine = Box::new(Engine::new());
            for depth in 1..=6 {
                let started = std::time::Instant::now();
                let result = engine.search(
                    &game,
                    &Limits {
                        max_depth: depth,
                        noise: 0,
                        seed: 0,
                    },
                    &mut |_| false,
                );
                let seconds = started.elapsed().as_secs_f64();
                std::println!(
                    "depth {depth}: {} nodes, {:.3} s, {:.0} nodes/s, {} {}",
                    result.nodes,
                    seconds,
                    f64::from(result.nodes) / seconds,
                    uci(&result),
                    result.score
                );
            }
        }
    }

    /// Engine against itself with a queen up: the mop-up term has to lead to
    /// mate well inside the 50-move rule.
    #[test]
    fn converts_king_and_queen_against_king() {
        let mut game = Game::new(Board::from_fen("8/8/8/4k3/8/8/8/4K2Q w - - 0 1").unwrap());
        let mut engine = Box::new(Engine::new());
        for _ in 0..80 {
            if game.outcome().is_some() {
                break;
            }
            let result = engine.search(
                &game,
                &Limits {
                    max_depth: 4,
                    noise: 0,
                    seed: 0,
                },
                &mut |_| false,
            );
            game.play(result.best.unwrap()).unwrap();
        }
        assert_eq!(
            game.outcome(),
            Some(Outcome::Checkmate {
                winner: crate::board::Color::White
            })
        );
    }
}
