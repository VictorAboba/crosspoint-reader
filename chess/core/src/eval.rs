//! Static evaluation in centipawns, from the side to move's point of view.
//!
//! Every term has a middlegame and an endgame value; the two are blended by
//! how much material is left. The square tables come from a few rules
//! (centralise the pieces, advance the pawns, keep the king sheltered until
//! the endgame) instead of typed-in numbers.

use crate::board::{Board, Color, PieceKind, Square};

// Indexed by `PieceKind as usize`; slot 0 is unused.
const MG_VALUE: [i32; 7] = [0, 100, 320, 335, 500, 950, 0];
const EG_VALUE: [i32; 7] = [0, 120, 300, 320, 540, 980, 0];
const PHASE_WEIGHT: [i32; 7] = [0, 0, 1, 1, 2, 4, 0];
const PHASE_TOTAL: i32 = 24;

// By the pawn's rank counted from its own side.
const PASSED_MG: [i32; 8] = [0, 5, 10, 15, 25, 40, 60, 0];
const PASSED_EG: [i32; 8] = [0, 10, 20, 35, 55, 85, 130, 0];
const DOUBLED: (i32, i32) = (-12, -20);
const ISOLATED: (i32, i32) = (-10, -15);
const BISHOP_PAIR: (i32, i32) = (30, 45);
const ROOK_OPEN_FILE: (i32, i32) = (20, 10);
const ROOK_HALF_OPEN_FILE: (i32, i32) = (10, 5);
const TEMPO: i32 = 10;

/// King steps from the centre: 0 on d4, d5, e4 and e5, up to 3 on the rim.
const fn rim(file: i32, rank: i32) -> i32 {
    let f = (2 * file - 7).abs() / 2;
    let r = (2 * rank - 7).abs() / 2;
    if f > r { f } else { r }
}

/// Middlegame and endgame bonus for a piece on `file` and `rank`, the rank
/// counted from the piece's own side.
const fn square_bonus(kind: usize, file: i32, rank: i32) -> (i32, i32) {
    let centre = 3 - rim(file, rank);
    let back_rank = if rank == 0 { 10 } else { 0 };
    match kind {
        1 => {
            // Pawns: advance, and take the centre early.
            let mut mg = 5 * (rank - 1);
            if (file == 3 || file == 4) && (rank == 3 || rank == 4) {
                mg += 15;
            }
            if (file == 2 || file == 5) && rank == 3 {
                mg += 5;
            }
            if (file == 3 || file == 4) && rank == 1 {
                mg -= 10;
            }
            (mg, 10 * (rank - 1))
        }
        2 => (12 * centre - 25 - back_rank, 8 * centre - 15),
        3 => (6 * centre - 8 - back_rank, 5 * centre - 6),
        4 => {
            let seventh = if rank == 6 { 20 } else { 0 };
            let centre_file = if file == 3 || file == 4 { 6 } else { 0 };
            (seventh + centre_file, seventh * 3 / 4)
        }
        5 => (3 * centre - 4, 6 * centre - 9),
        6 => {
            // The king hides behind its pawns until the endgame, then walks to the centre.
            const SHELTER: [i32; 8] = [20, 30, 10, 0, 0, 10, 30, 20];
            let mg = if rank == 0 {
                SHELTER[file as usize]
            } else {
                -25 * if rank < 3 { rank } else { 3 }
            };
            (mg, 14 * centre - 25)
        }
        _ => (0, 0),
    }
}

/// `[kind][square]` for White; Black reads the rank-mirrored square.
const TABLES: [[(i16, i16); 64]; 7] = {
    let mut tables = [[(0, 0); 64]; 7];
    let mut kind = 1;
    while kind < 7 {
        let mut square = 0;
        while square < 64 {
            let (mg, eg) = square_bonus(kind, (square % 8) as i32, (square / 8) as i32);
            tables[kind][square] = (mg as i16, eg as i16);
            square += 1;
        }
        kind += 1;
    }
    tables
};

#[derive(Default)]
struct Side {
    mg: i32,
    eg: i32,
    pawns: [u8; 8], // pawns per file
    material: i32,  // middlegame value of pieces other than pawns and king
    pawn_count: i32,
    bishops: i32,
    king: u8,
}

impl Side {
    fn add(&mut self, (mg, eg): (i32, i32)) {
        self.mg += mg;
        self.eg += eg;
    }
}

pub(crate) fn evaluate(board: &Board) -> i32 {
    if board.insufficient_material() {
        return 0;
    }

    let mut sides = [Side::default(), Side::default()];
    let mut phase = 0;
    // Per file, the highest Black pawn and the lowest White pawn (absolute
    // ranks): a pawn is passed when no enemy pawn stands ahead of it on its
    // own file or the two next to it.
    let mut black_highest = [-1i32; 8];
    let mut white_lowest = [8i32; 8];

    for square in Square::all() {
        let Some(piece) = board.piece_at(square) else {
            continue;
        };
        let kind = piece.kind as usize;
        let (file, rank) = (square.file() as usize, square.rank() as usize);
        let relative = match piece.color {
            Color::White => square.index() as usize,
            Color::Black => (7 - rank) * 8 + file,
        };
        let side = &mut sides[piece.color as usize];
        let (mg, eg) = TABLES[kind][relative];
        side.add((MG_VALUE[kind] + mg as i32, EG_VALUE[kind] + eg as i32));
        phase += PHASE_WEIGHT[kind];
        match piece.kind {
            PieceKind::Pawn => {
                side.pawns[file] += 1;
                side.pawn_count += 1;
                match piece.color {
                    Color::White => white_lowest[file] = white_lowest[file].min(rank as i32),
                    Color::Black => black_highest[file] = black_highest[file].max(rank as i32),
                }
            }
            PieceKind::King => side.king = square.index(),
            _ => {
                side.material += MG_VALUE[kind];
                if piece.kind == PieceKind::Bishop {
                    side.bishops += 1;
                }
            }
        }
    }

    // Pawn structure and rooks on open files need both sides' pawn files.
    for square in Square::all() {
        let Some(piece) = board.piece_at(square) else {
            continue;
        };
        let us = piece.color as usize;
        let (file, rank) = (square.file() as usize, square.rank() as i32);
        let neighbours = file.saturating_sub(1)..=(file + 1).min(7);
        match piece.kind {
            PieceKind::Pawn => {
                let own = &sides[us].pawns;
                let isolated =
                    (file == 0 || own[file - 1] == 0) && (file == 7 || own[file + 1] == 0);
                let (passed, relative) = match piece.color {
                    Color::White => (neighbours.clone().all(|f| black_highest[f] <= rank), rank),
                    Color::Black => (
                        neighbours.clone().all(|f| white_lowest[f] >= rank),
                        7 - rank,
                    ),
                };
                let side = &mut sides[us];
                if isolated {
                    side.add(ISOLATED);
                }
                if passed {
                    side.add((PASSED_MG[relative as usize], PASSED_EG[relative as usize]));
                }
            }
            PieceKind::Rook => {
                let (own, theirs) = (sides[us].pawns[file], sides[1 - us].pawns[file]);
                if own == 0 {
                    sides[us].add(if theirs == 0 {
                        ROOK_OPEN_FILE
                    } else {
                        ROOK_HALF_OPEN_FILE
                    });
                }
            }
            _ => {}
        }
    }

    for side in &mut sides {
        for count in side.pawns {
            if count > 1 {
                side.add((
                    DOUBLED.0 * (count as i32 - 1),
                    DOUBLED.1 * (count as i32 - 1),
                ));
            }
        }
        if side.bishops >= 2 {
            side.add(BISHOP_PAIR);
        }
    }

    let phase = phase.min(PHASE_TOTAL);
    let mg = sides[0].mg - sides[1].mg;
    let eg = sides[0].eg - sides[1].eg;
    let mut score = (mg * phase + eg * (PHASE_TOTAL - phase)) / PHASE_TOTAL;
    score += mop_up(&sides);

    match board.side_to_move() {
        Color::White => score + TEMPO,
        Color::Black => -score + TEMPO,
    }
}

/// Against a bare king, drive it to the edge and bring the own king close,
/// so the search finds mates that lie beyond its horizon.
fn mop_up(sides: &[Side; 2]) -> i32 {
    let bare = |side: &Side| side.material == 0 && side.pawn_count == 0;
    let (strong, weak, sign) = if bare(&sides[1]) && sides[0].material >= MG_VALUE[4] {
        (&sides[0], &sides[1], 1)
    } else if bare(&sides[0]) && sides[1].material >= MG_VALUE[4] {
        (&sides[1], &sides[0], -1)
    } else {
        return 0;
    };
    let (wf, wr) = ((weak.king % 8) as i32, (weak.king / 8) as i32);
    let (sf, sr) = ((strong.king % 8) as i32, (strong.king / 8) as i32);
    let distance = (wf - sf).abs() + (wr - sr).abs();
    sign * (40 * rim(wf, wr) + 8 * (14 - distance))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::STARTPOS_FEN;
    use std::string::String;
    use std::vec::Vec;

    /// The same position with the colours swapped and the board turned over.
    fn mirror(fen: &str) -> String {
        let fields: Vec<&str> = fen.split(' ').collect();
        let swap_case = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_uppercase() {
                        c.to_ascii_lowercase()
                    } else {
                        c.to_ascii_uppercase()
                    }
                })
                .collect()
        };
        let placement: Vec<String> = fields[0].split('/').rev().map(swap_case).collect();
        let side = if fields[1] == "w" { "b" } else { "w" };
        let mut castling: Vec<char> = swap_case(fields[2]).chars().collect();
        castling.sort_by_key(|c| (c.is_ascii_lowercase(), *c));
        let castling: String = castling.into_iter().collect();
        let en_passant = match fields[3] {
            "-" => String::from("-"),
            square => {
                let mut chars = square.chars();
                let file = chars.next().unwrap();
                let rank = if chars.next() == Some('3') { '6' } else { '3' };
                [file, rank].iter().collect()
            }
        };
        std::format!(
            "{} {side} {castling} {en_passant} {} {}",
            placement.join("/"),
            fields[4],
            fields[5]
        )
    }

    fn eval(fen: &str) -> i32 {
        evaluate(&Board::from_fen(fen).unwrap())
    }

    #[test]
    fn start_position_is_level() {
        assert_eq!(eval(STARTPOS_FEN), TEMPO);
    }

    #[test]
    fn colours_are_symmetric() {
        for fen in [
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "8/8/8/4k3/8/8/8/4K2Q w - - 0 1",
            "rnbqkbnr/ppp1pppp/8/8/3pP3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 3",
        ] {
            assert_eq!(eval(fen), eval(&mirror(fen)), "{fen} vs {}", mirror(fen));
        }
    }

    #[test]
    fn material_and_structure_point_the_right_way() {
        // White a queen up.
        assert!(eval("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1") > 800);
        // A far advanced passed pawn is worth more than a blocked one.
        assert!(
            eval("4k3/8/3P4/8/8/8/8/4K3 w - - 0 1")
                > eval("4k3/3p4/3P4/8/8/8/8/4K3 w - - 0 1") + 50
        );
        // Bare kings, or a lone minor piece, are a draw.
        assert_eq!(eval("4k3/8/8/8/8/8/8/4K1N1 w - - 0 1"), 0);
        // The mop-up term prefers the bare king on the edge.
        assert!(eval("7k/8/8/8/8/8/8/4K2Q w - - 0 1") > eval("8/8/8/4k3/8/8/8/4K2Q w - - 0 1"));
    }
}
