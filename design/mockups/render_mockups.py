#!/usr/bin/env python3
"""Render the CrossChess screen mockups at device pixel size.

Run from the repo root with the project venv (stdlib only):
  .venv/Scripts/python design/mockups/render_mockups.py

Writes 1-bit PNGs (1:1 and 2x) next to this file. Geometry constants mirror
design/README.md; the board and pieces use the same bitmaps as the firmware.
"""

from __future__ import annotations

from pathlib import Path

from epd import Canvas, font, pieces

OUT = Path(__file__).resolve().parent
SETS = pieces.render_all()
KINDS = "pnbrqk"  # CC_ kind - 1

# ---------------------------------------------------------------------------
# Layout (portrait 528 x 792), see design/README.md "Layout"
# ---------------------------------------------------------------------------
W, H = 528, 792
SQ = 64
BOARD_X = 8
TOP_PADDING = 10  # Lyra metrics.topPadding
HEADER_H = 56  # Lyra metrics.homeTopPadding: the short header band home uses
TOP_BAR_Y = HEADER_H + 4  # 60
BAR_H = 44
BOARD_Y = TOP_BAR_Y + BAR_H + 4  # 108
BOTTOM_BAR_Y = BOARD_Y + 8 * SQ + 4  # 624
STRIP_Y = BOTTOM_BAR_Y + BAR_H + 4  # 672, move strip down to the hints
STRIP_LINE = 34  # move strip line pitch: two lines fit
HINTS_H = 40
HINTS_Y = H - HINTS_H  # 752

# ---------------------------------------------------------------------------
# Positions
# ---------------------------------------------------------------------------
START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"


def sq(name: str) -> tuple[int, int]:
    return ord(name[0]) - 97, int(name[1]) - 1


def parse_fen(fen: str) -> dict[tuple[int, int], str]:
    board = {}
    for r, row in enumerate(fen.split()[0].split("/")):
        f = 0
        for ch in row:
            if ch.isdigit():
                f += int(ch)
            else:
                board[(f, 7 - r)] = ch
                f += 1
    return board


def play(moves: str, fen: str = START) -> tuple[dict, tuple[str, str] | None]:
    """Apply coordinate moves (e2e4, e1g1 castles, b7b8q promotes)."""
    board = parse_fen(fen)
    last = None
    for mv in moves.split():
        a, b = sq(mv[:2]), sq(mv[2:4])
        p = board.pop(a)
        if p in "Kk" and abs(b[0] - a[0]) == 2:
            rook_from = (7 if b[0] > a[0] else 0, a[1])
            board[((a[0] + b[0]) // 2, a[1])] = board.pop(rook_from)
        if len(mv) == 5:
            p = mv[4].upper() if p.isupper() else mv[4]
        board[b] = p
        last = (mv[:2], mv[2:4])
    return board, last


def captured(board: dict) -> tuple[list[str], list[str]]:
    """Pieces missing from each side, most valuable first."""
    order = "qrbnp"
    counts = {c: 0 for c in "pnbrqkPNBRQK"}
    for p in board.values():
        counts[p] += 1
    full = {"p": 8, "n": 2, "b": 2, "r": 2, "q": 1}
    lost_black = [k for k in order for _ in range(max(0, full[k] - counts[k]))]
    lost_white = [k.upper() for k in order for _ in range(max(0, full[k] - counts[k.upper()]))]
    return lost_white, lost_black


VALUE = {"p": 1, "n": 3, "b": 3, "r": 5, "q": 9}

# ---------------------------------------------------------------------------
# Board
# ---------------------------------------------------------------------------
BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]


def draw_piece(cv: Canvas, p: str, x: int, y: int, size: int = SQ, rotate: bool = False) -> None:
    glyph = SETS[size][KINDS.index(p.lower())]
    cv.piece(glyph, x, y, p.islower(), rotate)


def square_xy(f: int, r: int, flipped: bool, bx: int = BOARD_X, by: int = BOARD_Y, s: int = SQ):
    col, row = (7 - f, r) if flipped else (f, 7 - r)
    return bx + col * s, by + row * s


def draw_board(
    cv: Canvas,
    board: dict,
    *,
    bx: int = BOARD_X,
    by: int = BOARD_Y,
    s: int = SQ,
    flipped: bool = False,
    last: tuple[str, str] | None = None,
    check: str | None = None,
    cursor: str | None = None,
    selected: str | None = None,
    targets: tuple[str, ...] = (),
    coords: bool = True,
    hidden: tuple[str, ...] = (),
) -> None:
    cv.rect(bx - 2, by - 2, 8 * s + 4, 8 * s + 4, 2)
    marks = {sq(n) for n in last} if last else set()
    for r in range(8):
        for f in range(8):
            x, y = square_xy(f, r, flipped, bx, by, s)
            dark = (f + r) % 2 == 0
            cv.fill(x, y, s, s, "hatch" if dark else "white")
            if (f, r) in marks:
                # Last move: a 50% ring inside the square edge (s / 10 wide).
                ring = max(3, s // 10)
                for yy in range(s):
                    for xx in range(s):
                        if min(xx, yy, s - 1 - xx, s - 1 - yy) < ring:
                            cv.set(x + xx, y + yy, (x + xx + y + yy) % 2 == 0)
            name = f"{chr(97 + f)}{r + 1}"
            if name == check:
                # Radial ordered-dither glow: dense at the centre, fading out.
                for yy in range(s):
                    for xx in range(s):
                        d = ((xx - s / 2 + 0.5) ** 2 + (yy - s / 2 + 0.5) ** 2) ** 0.5 / (s / 2)
                        level = max(0.0, 1.0 - d) * 1.6
                        cv.set(x + xx, y + yy, BAYER[(y + yy) & 3][(x + xx) & 3] < level * 16)
            occupied = (f, r) in board and name not in hidden
            if name in targets and occupied:
                leg = s * 7 // 32
                for cx, cy, dx, dy in ((x, y, 1, 1), (x + s, y, -1, 1), (x, y + s, 1, -1), (x + s, y + s, -1, -1)):
                    cv.polygon([(cx, cy), (cx + dx * leg, cy), (cx, cy + dy * leg)])
            if occupied:
                draw_piece(cv, board[(f, r)], x, y, s)
            if name in targets and not occupied:
                cv.disc(x + s / 2, y + s / 2, s * 0.125 + 2, "white")
                cv.disc(x + s / 2, y + s / 2, s * 0.125)
            if name == selected:
                cv.rect(x, y, s, s, 4)
                cv.rect(x + 4, y + 4, s - 8, s - 8, 1, black=False)
            if name == cursor:
                arm, t = s // 4, 4
                for cx, cy, dx, dy in ((x, y, 1, 1), (x + s, y, -1, 1), (x, y + s, 1, -1), (x + s, y + s, -1, -1)):
                    x0 = cx if dx > 0 else cx - arm
                    y0 = cy if dy > 0 else cy - t
                    cv.fill(x0 - (1 if dx > 0 else -1), y0 - (1 if dy > 0 else -1), arm, t + 2, "white")
                for cx, cy, dx, dy in ((x, y, 1, 1), (x + s, y, -1, 1), (x, y + s, 1, -1), (x + s, y + s, -1, -1)):
                    cv.fill(cx if dx > 0 else cx - arm, cy if dy > 0 else cy - t, arm, t)
                    cv.fill(cx if dx > 0 else cx - t, cy if dy > 0 else cy - arm, t, arm)
    if coords and s >= 48:
        for i in range(8):
            f = 7 - i if flipped else i
            x, y = square_xy(f, 0 if not flipped else 7, flipped, bx, by, s)
            label = chr(97 + f)
            cv.text_halo("small", x + s - font("small").width(label) - 4, y + s - 19, label)
            r = 7 - i if not flipped else i
            x, y = square_xy(0 if not flipped else 7, r, flipped, bx, by, s)
            cv.text_halo("small", x + 4, y + 1, str(r + 1))


# ---------------------------------------------------------------------------
# Chrome
# ---------------------------------------------------------------------------


def battery(cv: Canvas, right: int, y: int, percent: int = 78) -> None:
    label = f"{percent}%"
    tw = font("small").width(label)
    cv.text("small", right - tw, y, label)
    bx = right - tw - 22
    cv.rect(bx, y + 4, 15, 11, 1)
    cv.fill(bx + 15, y + 7, 2, 5)
    cv.fill(bx + 2, y + 6, int(11 * percent / 100), 7)


def header(cv: Canvas, title: str, tall: bool = False, right: str | None = None) -> int:
    """Lyra-style header band (GUI.drawHeader). Returns its bottom y."""
    if tall:
        h, ty = 84, 40
        battery(cv, W - 14, 6)
    else:
        h = HEADER_H
        ty = TOP_PADDING + (h - TOP_PADDING - 3 - font("ui12b").advance_y) // 2
        battery(cv, W - 14, ty + 5)
    cv.text("ui12b", 18, ty, title)
    if right:
        cv.text_right("small", W - 90, ty + 6, right)
    cv.fill(0, h - 3, W, 3)
    return h


def hints(cv: Canvas, labels: tuple[str, str, str, str]) -> None:
    """LyraTheme::drawButtonHints on the 528 px wide X3 panel."""
    for x, label in zip((65, 157, 291, 383), labels):
        h = HINTS_H if label else 15
        y = H - h
        cv.round_fill(x, y, 80, h + 6, 6, "white")
        cv.round_rect(x, y, 80, h + 6, 6, 1)
        if label:
            cv.text_center("small", x + 40, y + (h - 19) // 2, label)


def side_ticks(cv: Canvas) -> None:
    """Up/Down live on the X3 edges (y 155..233): tiny arrows in the margin."""
    cv.polygon([(0, 198), (5, 198), (2.5, 191)])
    cv.polygon([(W - 5, 191), (W, 191), (W - 2.5, 198)])


def pill(cv: Canvas, right: int, y: int, label: str, inverted: bool = True, fname: str = "ui10b") -> int:
    w = font(fname).width(label) + 20
    h = 28
    cv.round_fill(right - w, y, w, h, 14, "black" if inverted else "white")
    if not inverted:
        cv.round_rect(right - w, y, w, h, 14, 2)
    cv.text(fname, right - w + 10, y + (h - font(fname).advance_y) // 2, label, black=not inverted)
    return w


def token(cv: Canvas, x: int, y: int, black: bool) -> None:
    """Side marker: which colour this bar belongs to."""
    cv.round_fill(x, y, 18, 18, 4, "black")
    if not black:
        cv.round_fill(x + 2, y + 2, 14, 14, 3, "white")


def player_bar(
    cv: Canvas,
    y: int,
    name: str,
    black: bool,
    taken: list[str],
    advantage: int = 0,
    chip: str | None = None,
    chip_inverted: bool = True,
    sub: str | None = None,
) -> None:
    token(cv, 14, y + 13, black)
    x = 42
    x += cv.text("ui12b" if chip_inverted and chip else "ui12", x, y + 7, name) + 12
    if sub:
        x += cv.text("ui10", x, y + 10, sub) + 10
    for i, p in enumerate(taken):
        prev = taken[i - 1] if i else p
        x += 0 if i == 0 else (11 if prev == p else 16)
        draw_piece(cv, p, x - 4, y + 9, 24)
    if taken:
        x += 18
    if advantage:
        cv.text("ui10", x, y + 11, f"+{advantage}")
    if chip:
        pill(cv, W - 12, y + 8, chip, chip_inverted)


def fit_tail(items: list[tuple[str, bool]], width: int, max_rows: int) -> list[tuple[str, bool]]:
    """Newest moves that fit in max_rows, starting at a move number."""
    starts = [i for i, (tok, _) in enumerate(items) if tok.endswith(".")]
    for start in starts:
        if figurine_moves(None, 0, 0, width, items[start:]) <= max_rows:
            return items[start:]
    return items[starts[-1] :]


def figurine_moves(cv: Canvas | None, x0: int, y: int, right: int, items: list[tuple[str, bool]], line_h: int = 34) -> int:
    """Move list with figurine SAN. items: (token, highlighted). Returns rows.

    cv=None only measures.
    """
    f = "ui12"
    x = x0
    rows = 1

    def token_width(tok: str) -> int:
        if tok[0] in "KQRBN":
            g = SETS[24][KINDS.index(tok[0].lower())]
            return g.width - 1 + font(f).width(tok[1:])
        return font(f).width(tok)

    for k, (tok, hl) in enumerate(items):
        w = token_width(tok) + (12 if hl else 0)
        gap = 6 if tok.endswith(".") else 14
        need = w
        if tok.endswith(".") and k + 1 < len(items):
            need += gap + token_width(items[k + 1][0]) + (12 if items[k + 1][1] else 0)
        if x + need > right and x > x0:
            x = x0
            y += line_h
            rows += 1
        tx = x + (6 if hl else 0)
        if cv is None:
            x += w + gap
            continue
        if hl:
            cv.round_fill(x, y - 1, w, 31, 6, "black")
        if tok[0] in "KQRBN":
            g = SETS[24][KINDS.index(tok[0].lower())]
            if hl:
                # Inverted figurine: white silhouette on the black pill.
                for j in range(g.height):
                    for i in range(g.width):
                        if g.black[j * g.width + i]:
                            cv.set(tx + i - 1, y + 3 + j + (21 - g.height), False)
            else:
                cv.piece(g, tx - g.left - 1, y + 2 - g.top + (21 - g.height) + g.top, False)
            tx += g.width - 1
            tok = tok[1:]
        cv.text(f, tx, y, tok, black=not hl)
        x += w + gap
    return rows


RU_LETTERS = {"K": "Кр", "Q": "Ф", "R": "Л", "B": "С", "N": "К"}  # Russian piece letters


def san_items(
    sans: list[str], first_move: int = 1, highlight: int | None = None, letters: bool = False
) -> list[tuple[str, bool]]:
    """Move tokens; letters=True spells pieces out instead of figurines."""
    items = []
    for i, san in enumerate(sans):
        if i % 2 == 0:
            items.append((f"{first_move + i // 2}.", False))
        if letters and san[0] in RU_LETTERS:
            san = RU_LETTERS[san[0]] + san[1:]
        items.append((san, i == highlight))
    return items


# ---------------------------------------------------------------------------
# Screens
# ---------------------------------------------------------------------------
TWO_KNIGHTS = (
    "e2e4 e7e5 g1f3 b8c6 f1c4 g8f6 f3g5 d7d5 e4d5 c6a5 c4b5 c7c6 d5c6 b7c6 b5e2 h7h6 g5f3 e5e4 f3e5 f8d6"
)
TWO_KNIGHTS_SAN = "e4 e5 Nf3 Nc6 Bc4 Nf6 Ng5 d5 exd5 Na5 Bb5+ c6 dxc6 bxc6 Be2 h6 Nf3 e4 Ne5 Bd6".split()


def screen_game(letters: bool = False) -> None:
    cv = Canvas()
    board, last = play(TWO_KNIGHTS)
    header(cv, "Партия", right="Ход 11")
    lost_white, lost_black = captured(board)
    adv = sum(VALUE[p] for p in lost_black) - sum(VALUE[p.lower()] for p in lost_white)
    player_bar(cv, TOP_BAR_Y, "Движок", True, lost_white, sub="уровень 3")
    draw_board(cv, board, last=last, cursor="e5")
    player_bar(cv, BOTTOM_BAR_Y, "Вы", False, lost_black, adv, chip="Ваш ход")
    cv.fill(12, STRIP_Y, W - 24, 1, "light")
    items = san_items(TWO_KNIGHTS_SAN, 1, highlight=len(TWO_KNIGHTS_SAN) - 1, letters=letters)
    figurine_moves(cv, 14, STRIP_Y + 8, W - 14, fit_tail(items, W - 28, 2), STRIP_LINE)
    hints(cv, ("Меню", "Выбрать", "Влево", "Вправо"))
    side_ticks(cv)
    save(cv, "2b_game_letters" if letters else "2_game")


def screen_move_select() -> None:
    cv = Canvas()
    board, last = play(TWO_KNIGHTS)
    header(cv, "Партия", right="Ход 11")
    lost_white, lost_black = captured(board)
    player_bar(cv, TOP_BAR_Y, "Движок", True, lost_white, sub="уровень 3")
    targets = ("c4", "c6", "d3", "d7", "f3", "f7", "g4", "g6")
    draw_board(cv, board, last=last, cursor="f7", selected="e5", targets=targets)
    player_bar(cv, BOTTOM_BAR_Y, "Вы", False, lost_black, 1, chip="Конь e5–f7")
    cv.fill(12, STRIP_Y, W - 24, 1, "light")
    cv.text("ui12", 14, STRIP_Y + 8, "Конь берёт пешку f7")
    cv.text("ui10", 14, STRIP_Y + 42, "8 ходов · стрелки прыгают по целям хода")
    hints(cv, ("Отмена", "Сходить", "Влево", "Вправо"))
    side_ticks(cv)
    save(cv, "3_move_select")


def screen_promotion() -> None:
    cv = Canvas()
    fen = "4r1k1/1P3ppp/8/8/8/8/5PPP/3R2K1"
    board = parse_fen(fen)
    header(cv, "Партия", right="Ход 41")
    player_bar(cv, TOP_BAR_Y, "Движок", True, ["Q", "R", "B", "B", "N", "N", "P", "P", "P", "P"], sub="уровень 3")
    draw_board(cv, board, last=("e2", "e8"), selected="b7", targets=("b8",), hidden=())
    # Picker: a column of the four choices growing down from the promotion square.
    x, y = square_xy(*sq("b8"), False)
    cv.fill(x + 6, y + 6, SQ, 4 * SQ, "black")  # drop shadow
    cv.fill(x - 2, y - 2, SQ + 4, 4 * SQ + 4, "black")
    for i, p in enumerate("QNRB"):
        cy = y + i * SQ
        cv.fill(x, cy, SQ, SQ, "white")
        draw_piece(cv, p, x, cy)
        if i:
            cv.fill(x + 6, cy, SQ - 12, 1, "light")
    arm, t = SQ // 4, 4
    for cx, cy, dx, dy in ((x, y, 1, 1), (x + SQ, y, -1, 1), (x, y + SQ, 1, -1), (x + SQ, y + SQ, -1, -1)):
        cv.fill(cx if dx > 0 else cx - arm, cy if dy > 0 else cy - t, arm, t)
        cv.fill(cx if dx > 0 else cx - t, cy if dy > 0 else cy - arm, t, arm)
    player_bar(cv, BOTTOM_BAR_Y, "Вы", False, ["q", "r", "b", "b", "n", "n", "p", "p", "p", "p", "p"], 4, chip="Превращение")
    cv.fill(12, STRIP_Y, W - 24, 1, "light")
    cv.text("ui12", 14, STRIP_Y + 8, "Пешка b7–b8: выберите фигуру")
    cv.text("ui10", 14, STRIP_Y + 42, "Ферзь · Конь · Ладья · Слон")
    hints(cv, ("Отмена", "Выбрать", "Вверх", "Вниз"))
    side_ticks(cv)
    save(cv, "4_promotion")


OPERA = (
    "e2e4 e7e5 g1f3 d7d6 d2d4 c8g4 d4e5 g4f3 d1f3 d6e5 f1c4 g8f6 f3b3 d8e7 b1c3 c7c6 c1g5 b7b5 "
    "c3b5 c6b5 c4b5 b8d7 e1c1 a8d8 d1d7 d8d7 h1d1 e7e6 b5d7 f6d7 b3b8 d7b8 d1d8"
)
OPERA_SAN = (
    "e4 e5 Nf3 d6 d4 Bg4 dxe5 Bxf3 Qxf3 dxe5 Bc4 Nf6 Qb3 Qe7 Nc3 c6 Bg5 b5 Nxb5 cxb5 "
    "Bxb5+ Nbd7 O-O-O Rd8 Rxd7 Rxd7 Rd1 Qe6 Bxd7+ Nxd7 Qb8+ Nxb8 Rd8#"
).split()


def screen_game_over() -> None:
    cv = Canvas()
    board, last = play(OPERA)
    header(cv, "Партия", right="Мат")
    lost_white, lost_black = captured(board)
    player_bar(cv, TOP_BAR_Y, "Движок", True, lost_white, sub="уровень 3")
    draw_board(cv, board, last=last, check="e8")
    player_bar(cv, BOTTOM_BAR_Y, "Вы", False, lost_black, 0, chip="1–0")
    cv.fill(12, STRIP_Y, W - 24, 1, "light")
    items = san_items(OPERA_SAN, 1, highlight=len(OPERA_SAN) - 1)
    figurine_moves(cv, 14, STRIP_Y + 8, W - 14, fit_tail(items, W - 28, 2), STRIP_LINE)

    # Dialog card over the board: 3 px frame, hard drop shadow, Lyra radius.
    dx, dy, dw, dh = 44, 176, 440, 360
    cv.round_fill(dx + 6, dy + 6, dw, dh, 8, "black")
    cv.round_fill(dx, dy, dw, dh, 8, "white")
    cv.round_rect(dx, dy, dw, dh, 8, 3)
    # Winner's king stands, loser's king lies on the same baseline.
    king = SETS[SQ][KINDS.index("k")]
    draw_piece(cv, "K", dx + 56, dy + 18)
    draw_piece(cv, "k", dx + dw - 132, dy + 18 + king.top + king.height - king.left - king.width, rotate=True)
    cv.text_center("big", W // 2, dy + 22, "1–0")
    cv.text_center("ui12b", W // 2, dy + 104, "Мат! Вы победили")
    cv.text_center("ui10", W // 2, dy + 136, "17 ходов · белыми · движок ур. 3")
    cv.fill(dx + 24, dy + 174, dw - 48, 1, "light")
    rows = ["Новая партия", "Реванш за чёрных", "Разбор партии", "В меню"]
    for i, label in enumerate(rows):
        ry = dy + 190 + i * 40
        if i == 0:
            cv.round_fill(dx + 16, ry, dw - 32, 34, 6, "light")
        cv.text("ui12", dx + 32, ry + 3, label)
    hints(cv, ("В меню", "Выбрать", "Вверх", "Вниз"))
    save(cv, "5_game_over")


def screen_puzzle() -> None:
    cv = Canvas()
    fen = "r5k1/pp1q1p1p/8/3N4/8/8/PPP2PPP/3R2K1"
    board, last = play("d5f6 g8g7", fen)
    header(cv, "Задачи", right="Рейтинг 1650")
    # Task bar: who moves + progress through the solution.
    token(cv, 14, TOP_BAR_Y + 13, False)
    cv.text("ui12b", 42, TOP_BAR_Y + 7, "Белые: выиграйте материал")
    for i in range(3):
        cx = W - 24 - (2 - i) * 22
        if i == 0:
            cv.disc(cx, TOP_BAR_Y + 22, 7)
        else:
            cv.disc(cx, TOP_BAR_Y + 22, 7)
            cv.disc(cx, TOP_BAR_Y + 22, 5, "white" if i == 2 else "light")
    draw_board(cv, board, last=last, cursor="f6")
    # Feedback bar: check mark + verdict, streak on the right.
    y = BOTTOM_BAR_Y
    cv.round_fill(14, y + 9, 26, 26, 13, "black")
    cv.line(20, y + 22, 25, y + 27, 3, black=False)
    cv.line(25, y + 27, 34, y + 16, 3, black=False)
    cv.text("ui12b", 50, y + 7, "Верно! Продолжайте")
    pill(cv, W - 12, y + 8, "Серия 7", inverted=False)
    cv.fill(12, STRIP_Y, W - 24, 1, "light")
    cv.text("ui12", 14, STRIP_Y + 8, "Задача #4821 · вилка, связка")
    cv.text("ui10", 14, STRIP_Y + 42, "Тема откроется после решения")
    hints(cv, ("Меню", "Выбрать", "Влево", "Вправо"))
    side_ticks(cv)
    save(cv, "6_puzzle")


def screen_pgn() -> None:
    cv = Canvas()
    ply = len(OPERA_SAN) - 2  # after 16. Qb8+
    board, last = play(" ".join(OPERA.split()[:ply]))
    header(cv, "PGN", right="Париж, 1858")
    lost_white, lost_black = captured(board)
    player_bar(cv, TOP_BAR_Y, "Брауншвейг, Изуар", True, lost_white)
    draw_board(cv, board, last=last)
    player_bar(cv, BOTTOM_BAR_Y, "Морфи", False, lost_black, 0)
    # Progress through the game under the white bar.
    cv.fill(12, STRIP_Y, W - 24, 1, "light")
    cv.fill(12, STRIP_Y - 1, int((W - 24) * ply / len(OPERA_SAN)), 3)
    items = san_items(OPERA_SAN[:ply], 1, highlight=ply - 1)
    figurine_moves(cv, 14, STRIP_Y + 8, W - 14, fit_tail(items, W - 28, 1), STRIP_LINE)
    cv.text("ui10", 14, STRIP_Y + 42, "Жертва ферзя: мат следующим ходом")
    cv.text_right("small", W - 14, STRIP_Y + 46, f"ход 16 · {ply}/{len(OPERA_SAN)}")
    hints(cv, ("« Назад", "Авто", "Пред.", "След."))
    side_ticks(cv)
    save(cv, "7_pgn")


def screen_hub() -> None:
    cv = Canvas()
    top = header(cv, "Шахматы", tall=True)
    board, _ = play(TWO_KNIGHTS)
    # Resume card with a 24 px mini board of the saved game (selected).
    cx, cy, cw, ch = 20, top + 14, W - 40, 224
    cv.round_fill(cx, cy, cw, ch, 6, "light")
    cv.fill(cx + 16, cy + 16, 192, 192, "white")
    draw_board(cv, board, bx=cx + 16, by=cy + 16, s=24, last=("f8", "d6"), coords=False)
    tx = cx + 232
    cv.text("ui12b", tx, cy + 18, "Продолжить")
    cv.text("ui10", tx, cy + 54, "Вы белыми · ход 11")
    cv.text("ui10", tx, cy + 80, "Движок, уровень 3")
    cv.text("ui10", tx, cy + 106, "Материал: +1")
    w = pill(cv, cx + cw - 16, cy + ch - 46, "Ваш ход")
    _ = w
    rows = [
        ("N", "Новая партия", "движок, уровни 1–8"),
        ("Q", "Задачи", "1650 · серия 7"),
        ("R", "Разбор PGN", "/chess/games"),
        (None, "Настройки", "доска, ввод"),
    ]
    y = cy + ch + 16
    for icon, label, sub in rows:
        if icon:
            draw_piece(cv, icon, 36, y + 18, 24)
        else:
            gear(cv, 48, y + 32)
        cv.text("ui12", 72, y + 17, label)
        cv.text_right("small", W - 36, y + 22, sub)
        y += 64 + 8
        cv.fill(36, y - 4, W - 72, 1, "light")
    cv.text_center("small", W // 2, HINTS_Y - 34, "Партий 32 · побед 19 · задач решено 214")
    hints(cv, ("« Назад", "Выбрать", "Вверх", "Вниз"))
    save(cv, "1_hub")


def gear(cv: Canvas, cx: int, cy: int) -> None:
    import math

    for i in range(8):
        a = i * math.pi / 4
        cv.line(cx + 7 * math.cos(a), cy + 7 * math.sin(a), cx + 10 * math.cos(a), cy + 10 * math.sin(a), 4)
    cv.disc(cx, cy, 8)
    cv.disc(cx, cy, 3.5, "white")


def move_table(cv: Canvas, x: int, y: int, sans: list[str], rows: int, current: int) -> None:
    """Two-column move list for the landscape side panel."""
    first = max(0, (len(sans) + 1) // 2 - rows)
    for row in range(first, min(first + rows, (len(sans) + 1) // 2)):
        ry = y + (row - first) * 32
        if row > first:
            cv.fill(x + 36, ry - 1, 178, 1, "light")
        cv.text_right("ui10", x + 30, ry + 4, f"{row + 1}.")
        for col in range(2):
            i = row * 2 + col
            if i >= len(sans):
                break
            figurine_moves(cv, x + 40 + col * 88, ry + 1, x + 214, [(sans[i], i == current)])


def screen_landscape() -> None:
    """Same game, landscape 792 x 528: board left, side panel right.

    The button hints stay on the physical button edge, drawn in portrait
    orientation by the theme; here that edge is on the right.
    """
    cv = Canvas(792, 528)
    board, last = play(TWO_KNIGHTS)
    draw_board(cv, board, bx=8, by=8, last=last, cursor="e5")
    px, pr = 8 * SQ + 18, 792 - HINTS_H - 10  # panel 530..742
    pw = pr - px
    cv.text("ui12b", px, 7, "Партия")
    battery(cv, pr, 12)
    cv.fill(px, 41, pr - px, 3)
    lost_white, lost_black = captured(board)
    token(cv, px, 62, True)
    cv.text("ui12", px + 28, 56, "Движок")
    cv.text("ui10", px + 28, 86, "уровень 3")
    for i, p in enumerate(lost_white):
        draw_piece(cv, p, px + 130 + i * 14, 84, 24)
    move_table(cv, px, 120, TWO_KNIGHTS_SAN, 8, len(TWO_KNIGHTS_SAN) - 1)
    token(cv, px, 400, False)
    cv.text("ui12b", px + 28, 394, "Вы")
    for i, p in enumerate(lost_black):
        draw_piece(cv, p, px + 64 + i * 11, 396, 24)
    cv.text("ui10", px + 110, 398, "+1")
    pill(cv, pr, 440, "Ваш ход")
    cv.text("small", px, 486, "Ход 11 · вы белыми")
    # Hints on the right edge, in panel order top to bottom.
    for y0, label in zip((65, 157, 291, 383), ("Меню", "Выбрать", "Влево", "Вправо")):
        cv.round_fill(792 - HINTS_H, y0, HINTS_H + 6, 80, 6, "white")
        cv.round_rect(792 - HINTS_H, y0, HINTS_H + 6, 80, 6, 1)
        cv.text_rotated("small", 792 - HINTS_H + 10, y0 + 40, label)
    save(cv, "8_game_landscape")


def save(cv: Canvas, name: str) -> None:
    cv.save(OUT / f"{name}.png")
    cv.save(OUT / f"{name}@2x.png", 2)
    print(f"wrote design/mockups/{name}.png")


def main() -> None:
    screen_hub()
    screen_game()
    screen_game(letters=True)
    screen_move_select()
    screen_promotion()
    screen_game_over()
    screen_puzzle()
    screen_pgn()
    screen_landscape()


if __name__ == "__main__":
    main()
