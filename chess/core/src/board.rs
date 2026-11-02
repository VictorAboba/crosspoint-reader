//! Board state and FEN parsing.

use core::fmt;

pub const STARTPOS_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    pub const fn opposite(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PieceKind {
    Pawn = 1,
    Knight = 2,
    Bishop = 3,
    Rook = 4,
    Queen = 5,
    King = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceKind,
}

impl Piece {
    pub const fn new(color: Color, kind: PieceKind) -> Self {
        Self { color, kind }
    }

    /// Compact code shared with the C API: kind in bits 0-2, bit 3 set for
    /// black. 0 is reserved for an empty square.
    pub const fn code(self) -> u8 {
        self.kind as u8 | (self.color as u8) << 3
    }

    pub const fn from_fen_char(c: char) -> Option<Piece> {
        let color = if c.is_ascii_uppercase() {
            Color::White
        } else {
            Color::Black
        };
        let kind = match c.to_ascii_lowercase() {
            'p' => PieceKind::Pawn,
            'n' => PieceKind::Knight,
            'b' => PieceKind::Bishop,
            'r' => PieceKind::Rook,
            'q' => PieceKind::Queen,
            'k' => PieceKind::King,
            _ => return None,
        };
        Some(Piece::new(color, kind))
    }

    pub const fn fen_char(self) -> char {
        let c = match self.kind {
            PieceKind::Pawn => 'p',
            PieceKind::Knight => 'n',
            PieceKind::Bishop => 'b',
            PieceKind::Rook => 'r',
            PieceKind::Queen => 'q',
            PieceKind::King => 'k',
        };
        match self.color {
            Color::White => c.to_ascii_uppercase(),
            Color::Black => c,
        }
    }
}

/// Board square: a1 = 0, b1 = 1, ..., h8 = 63.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Square(u8);

impl Square {
    pub const fn new(index: u8) -> Option<Square> {
        if index < 64 {
            Some(Square(index))
        } else {
            None
        }
    }

    pub const fn from_file_rank(file: u8, rank: u8) -> Option<Square> {
        if file < 8 && rank < 8 {
            Some(Square(rank * 8 + file))
        } else {
            None
        }
    }

    /// For indices already known to be on the board, such as the bits of a
    /// packed move.
    pub(crate) const fn from_index(index: u8) -> Square {
        Square(index & 63)
    }

    /// The square `files` and `ranks` away, if it is on the board.
    pub const fn offset(self, files: i8, ranks: i8) -> Option<Square> {
        let file = self.file() as i8 + files;
        let rank = self.rank() as i8 + ranks;
        if file >= 0 && file < 8 && rank >= 0 && rank < 8 {
            Some(Square((rank * 8 + file) as u8))
        } else {
            None
        }
    }

    pub const fn index(self) -> u8 {
        self.0
    }

    pub const fn file(self) -> u8 {
        self.0 % 8
    }

    pub const fn rank(self) -> u8 {
        self.0 / 8
    }

    pub fn all() -> impl Iterator<Item = Square> {
        (0..64).map(Square)
    }

    /// Parses algebraic coordinates such as "e4".
    pub fn parse(s: &str) -> Option<Square> {
        match s.as_bytes() {
            &[file, rank] => {
                Square::from_file_rank(file.wrapping_sub(b'a'), rank.wrapping_sub(b'1'))
            }
            _ => None,
        }
    }
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            (b'a' + self.file()) as char,
            (b'1' + self.rank()) as char
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CastlingRights(u8);

impl CastlingRights {
    pub const WHITE_KINGSIDE: u8 = 1;
    pub const WHITE_QUEENSIDE: u8 = 2;
    pub const BLACK_KINGSIDE: u8 = 4;
    pub const BLACK_QUEENSIDE: u8 = 8;

    const FEN_FLAGS: [(char, u8); 4] = [
        ('K', Self::WHITE_KINGSIDE),
        ('Q', Self::WHITE_QUEENSIDE),
        ('k', Self::BLACK_KINGSIDE),
        ('q', Self::BLACK_QUEENSIDE),
    ];

    pub const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn without(self, flags: u8) -> CastlingRights {
        CastlingRights(self.0 & !flags)
    }

    pub(crate) const fn bits(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenError {
    Placement,
    SideToMove,
    Castling,
    EnPassant,
    Clock,
    /// Each side must have exactly one king.
    Kings,
    TrailingData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub(crate) squares: [Option<Piece>; 64],
    /// King squares by color, kept in sync with `squares` for check tests.
    pub(crate) kings: [Square; 2],
    pub(crate) side_to_move: Color,
    pub(crate) castling: CastlingRights,
    pub(crate) en_passant: Option<Square>,
    pub(crate) halfmove_clock: u16,
    pub(crate) fullmove_number: u16,
}

impl Board {
    pub fn startpos() -> Board {
        match Board::from_fen(STARTPOS_FEN) {
            Ok(board) => board,
            Err(_) => unreachable!("STARTPOS_FEN is valid"),
        }
    }

    /// Parses a FEN string. The castling, en passant and clock fields may be
    /// omitted (as in EPD); they default to "- - 0 1".
    pub fn from_fen(fen: &str) -> Result<Board, FenError> {
        let mut fields = fen.split_ascii_whitespace();
        let placement = fields.next().ok_or(FenError::Placement)?;
        let side = fields.next().ok_or(FenError::SideToMove)?;
        let castling = fields.next().unwrap_or("-");
        let en_passant = fields.next().unwrap_or("-");
        let halfmove = fields.next().unwrap_or("0");
        let fullmove = fields.next().unwrap_or("1");
        if fields.next().is_some() {
            return Err(FenError::TrailingData);
        }

        let squares = parse_placement(placement)?;
        let mut kings = [Square(0); 2];
        for square in Square::all() {
            if let Some(Piece {
                color,
                kind: PieceKind::King,
            }) = squares[square.index() as usize]
            {
                kings[color as usize] = square;
            }
        }
        let side_to_move = match side {
            "w" => Color::White,
            "b" => Color::Black,
            _ => return Err(FenError::SideToMove),
        };

        Ok(Board {
            squares,
            kings,
            side_to_move,
            castling: parse_castling(castling)?,
            en_passant: parse_en_passant(en_passant, side_to_move)?,
            halfmove_clock: halfmove.parse().map_err(|_| FenError::Clock)?,
            fullmove_number: fullmove.parse().map_err(|_| FenError::Clock)?,
        })
    }

    pub fn piece_at(&self, square: Square) -> Option<Piece> {
        self.squares[square.index() as usize]
    }

    pub fn side_to_move(&self) -> Color {
        self.side_to_move
    }

    pub fn castling(&self) -> CastlingRights {
        self.castling
    }

    pub fn en_passant(&self) -> Option<Square> {
        self.en_passant
    }

    pub fn halfmove_clock(&self) -> u16 {
        self.halfmove_clock
    }

    pub fn fullmove_number(&self) -> u16 {
        self.fullmove_number
    }
}

fn parse_placement(placement: &str) -> Result<[Option<Piece>; 64], FenError> {
    let mut squares = [None; 64];
    let mut kings = [0u8; 2];
    let mut ranks = placement.split('/');

    for rank in (0..8u8).rev() {
        let row = ranks.next().ok_or(FenError::Placement)?;
        let mut file = 0u8;
        for c in row.chars() {
            if let Some(skip) = c.to_digit(10).filter(|d| (1..=8).contains(d)) {
                file += skip as u8;
                if file > 8 {
                    return Err(FenError::Placement);
                }
                continue;
            }
            let piece = Piece::from_fen_char(c).ok_or(FenError::Placement)?;
            let square = Square::from_file_rank(file, rank).ok_or(FenError::Placement)?;
            if piece.kind == PieceKind::Pawn && (rank == 0 || rank == 7) {
                return Err(FenError::Placement);
            }
            if piece.kind == PieceKind::King {
                kings[piece.color as usize] += 1;
            }
            squares[square.index() as usize] = Some(piece);
            file += 1;
        }
        if file != 8 {
            return Err(FenError::Placement);
        }
    }
    if ranks.next().is_some() {
        return Err(FenError::Placement);
    }
    if kings != [1, 1] {
        return Err(FenError::Kings);
    }
    Ok(squares)
}

fn parse_castling(field: &str) -> Result<CastlingRights, FenError> {
    if field == "-" {
        return Ok(CastlingRights::default());
    }
    let mut rights = 0u8;
    for c in field.chars() {
        let (_, flag) = CastlingRights::FEN_FLAGS
            .iter()
            .find(|(fen_char, _)| *fen_char == c)
            .ok_or(FenError::Castling)?;
        rights |= flag;
    }
    Ok(CastlingRights(rights))
}

fn parse_en_passant(field: &str, side_to_move: Color) -> Result<Option<Square>, FenError> {
    if field == "-" {
        return Ok(None);
    }
    let square = Square::parse(field).ok_or(FenError::EnPassant)?;
    // The target square lies behind a pawn that just made a double step.
    let expected_rank = match side_to_move {
        Color::White => 5,
        Color::Black => 2,
    };
    if square.rank() != expected_rank {
        return Err(FenError::EnPassant);
    }
    Ok(Some(square))
}

/// Formats the board as FEN.
impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for rank in (0..8u8).rev() {
            let mut empty = 0;
            for file in 0..8u8 {
                let square = Square(rank * 8 + file);
                match self.piece_at(square) {
                    Some(piece) => {
                        if empty > 0 {
                            write!(f, "{empty}")?;
                            empty = 0;
                        }
                        write!(f, "{}", piece.fen_char())?;
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                write!(f, "{empty}")?;
            }
            if rank > 0 {
                f.write_str("/")?;
            }
        }

        let side = match self.side_to_move {
            Color::White => "w",
            Color::Black => "b",
        };
        write!(f, " {side} ")?;

        if self.castling.is_empty() {
            f.write_str("-")?;
        }
        for (fen_char, flag) in CastlingRights::FEN_FLAGS {
            if self.castling.contains(flag) {
                write!(f, "{fen_char}")?;
            }
        }

        match self.en_passant {
            Some(square) => write!(f, " {square}")?,
            None => f.write_str(" -")?,
        }
        write!(f, " {} {}", self.halfmove_clock, self.fullmove_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;

    const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

    fn sq(name: &str) -> Square {
        Square::parse(name).unwrap()
    }

    #[test]
    fn startpos_has_pieces_in_place() {
        let board = Board::startpos();
        assert_eq!(
            board.piece_at(sq("e1")),
            Some(Piece::new(Color::White, PieceKind::King))
        );
        assert_eq!(
            board.piece_at(sq("d8")),
            Some(Piece::new(Color::Black, PieceKind::Queen))
        );
        assert_eq!(board.piece_at(sq("e4")), None);
        assert_eq!(board.side_to_move(), Color::White);
        assert_eq!(board.en_passant(), None);
    }

    #[test]
    fn fen_round_trips() {
        for fen in [
            STARTPOS_FEN,
            KIWIPETE,
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
        ] {
            assert_eq!(Board::from_fen(fen).unwrap().to_string(), fen);
        }
    }

    #[test]
    fn missing_fields_get_defaults() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 b").unwrap();
        assert_eq!(board.to_string(), "4k3/8/8/8/8/8/8/4K3 b - - 0 1");
    }

    #[test]
    fn rejects_invalid_fen() {
        let cases = [
            ("", FenError::Placement),
            ("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP w", FenError::Placement),
            (
                "rnbqkbnr/pppppppp/9/8/8/8/PPPPPPPP/RNBQKBNR w",
                FenError::Placement,
            ),
            (
                "rnbqkbnr/ppppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w",
                FenError::Placement,
            ),
            ("P3k3/8/8/8/8/8/8/4K3 w", FenError::Placement),
            ("4k3/8/8/8/8/8/8/4KK2 w", FenError::Kings),
            ("8/8/8/8/8/8/8/4K3 w", FenError::Kings),
            ("4k3/8/8/8/8/8/8/4K3 x", FenError::SideToMove),
            ("4k3/8/8/8/8/8/8/4K3 w KX", FenError::Castling),
            ("4k3/8/8/8/8/8/8/4K3 w - e3", FenError::EnPassant),
            ("4k3/8/8/8/8/8/8/4K3 w - - x 1", FenError::Clock),
            (
                "4k3/8/8/8/8/8/8/4K3 w - - 0 1 extra",
                FenError::TrailingData,
            ),
        ];
        for (fen, error) in cases {
            assert_eq!(Board::from_fen(fen), Err(error), "{fen}");
        }
    }

    #[test]
    fn piece_codes_match_c_api() {
        assert_eq!(Piece::new(Color::White, PieceKind::Pawn).code(), 1);
        assert_eq!(Piece::new(Color::White, PieceKind::King).code(), 6);
        assert_eq!(Piece::new(Color::Black, PieceKind::Pawn).code(), 9);
        assert_eq!(Piece::new(Color::Black, PieceKind::King).code(), 14);
    }
}
