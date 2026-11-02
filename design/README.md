# CrossChess visual and interaction design

Design for the chess app on the Xteink X3 (ESP32-C3, 792×528 panel, buttons
only). It also covers the X4 (480×800) because the `default` firmware build
runs on both.

**Status.** Built: the game screen with three kinds of game (against the
computer, training, two players), the game menu and the result card. The
rest is the target design and not built yet: the hub (the game opens from the
home list for now), puzzles, the PGN viewer, the notation setting (figurines
only for now), landscape, clocks, and offering a draw or resigning. Where the
sections below describe those, they are plans.

| Path | What it is |
| --- | --- |
| `design/pieces/*.svg` | Source of the "Plate" piece set (original drawing) |
| `design/pieces/preview/` | Generated contact sheets: 1-bit and 2-bit, on light and dark squares |
| `scripts/gen_chess_pieces.py` | SVG → `src/activities/chess/ChessPieceBitmaps.h` (stdlib Python only) |
| `src/activities/chess/ChessPieceBitmaps.h` | Generated bitmaps, 64/56/24 px, 9,843 bytes of flash |
| `design/mockups/*.png` | Screen mockups at device size (1:1 and `@2x`); open `index.html` |
| `design/mockups/render_mockups.py`, `epd.py` | Mockup renderer that uses the firmware fonts and bitmaps |

Regenerate everything from the repo root:

```bash
.venv/Scripts/python scripts/gen_chess_pieces.py
.venv/Scripts/python design/mockups/render_mockups.py
```

`gen_chess_pieces.py --check` exits 1 when the header does not match the SVGs.

## Decisions (2026-11-01)

These were open questions; the owner decided them.

| Question | Decision |
| --- | --- |
| Cursor | Smart cursor only: arrows jump between your own pieces, then between the legal targets of the picked piece. No one-square free cursor. |
| Back in a game | Back opens the game menu or cancels; a 1 s long press leaves the game (section 5). |
| Clocks | Shown only in games with a time control; they change once a minute at most (section 6). |
| Notation | A setting: figurines (default) or letters in the UI language (section 3). |
| Solid finials | Kept as the set's signature (section 2). |
| Opening names | Not in the first version (section 3). |

## 1. Design language: "Plate"

The look comes from printed chess diagrams in books, not from a colour screen
made gray. On 1-bit e-ink that look is native.

- **Paper and ink.** Light squares are paper white. Dark squares are
  *hatched*: 1 px diagonal lines every 4 px (25% ink), as engravers did.
  Hatching is crisp on the panel, it survives fast refresh, and it never
  looks like a dither mistake.
- **Pieces sit in a halo.** Every piece is drawn on a 2 px white ring cut out
  of the square. So a piece reads the same on paper, on hatch, on a
  highlight or on the check glow.
- **One stroke weight.** White pieces have a 2 px outline at 64 px. Black
  pieces are solid with a 2 px black rim and white inner lines. Board frame
  2 px. Selection frames 4 px.
- **Gray means history, black means now.** Passive state (last move) uses a
  50% dither. Active state (cursor, selection, move targets) uses solid black.
  So the eye finds "where am I" first.
- **Native chrome.** Header, button hints, menus, pills and popups follow the
  Lyra theme: 6 px corner radius, 3 px header rule, light-gray selection pill,
  Ubuntu UI fonts. The chess screens look like part of CrossPoint.

### Tones the panel can show (BW pass)

| Tone | Pattern | Used for |
| --- | --- | --- |
| White | — | light squares, halos, cards |
| Black | — | ink, cursor, selection, active pills |
| Hatch | `(x + y) % 4 == 0` | dark squares |
| Light gray | `Color::LightGray` (`x % 2 == 0 && y % 2 == 0`, 25%) | Lyra selection pill, separators |
| Dark gray | `Color::DarkGray` checker (50%) | last-move ring |
| Glow | 4×4 Bayer threshold, radial | king in check |

Patterns use screen coordinates, so they line up across squares.

## 2. Piece set

Six original shapes on a 64×64 unit grid (one unit is one pixel at the X3
board size, so the 64 px set is pixel-exact). All pieces share one plinth,
which ties the set together. They are built from rects, circles and simple
curves, not traced from any existing set.

- **Signature: solid finials.** Ball tips, the bishop's knob and the king's
  cross are solid black on both colours. This keeps tiny details readable and
  makes the set recognizable.
- **Heights** (64 px): king 59, queen and bishop 57, knight 55, rook 52,
  pawn 44 (including the halo).
- **Layers in each SVG** (contract in `pawn.svg`):
  - `body` — the filled silhouette.
  - `cut` — removed from the body (rook crenels).
  - `detail` — interior lines: black on white pieces, white on black pieces.
  - `detail-white` — ink on the white piece only (the finials).

  `data-small="keep"` marks the details that stay below 40 px.

The generator, not the SVG, makes the outline, halo and rim. It does them with
pixel morphology at the target size, so every size gets exact 2 px (or 1 px)
strokes.

| Size | Used for | Outline / halo |
| --- | --- | --- |
| 64 px | X3 board, promotion picker, game-over dialog | 2 / 2 px |
| 56 px | X4 board (squares of 58 px) | 2 / 2 px |
| 24 px | captured material, figurine move list, hub mini board | 1 / 1 px |

2-bit gray: `--gray` also writes `ChessPieceBitmapsGray.h` (64 px anti-aliased
ink, 6,770 bytes). It is not used. The game should stay 1-bit (see section 5),
and `drawGlyphBitmap` paints a 1-bit halo as "update" on the gray planes, so a
gray pass would have to draw halos in the BW pass only.

## 2a. How the bitmaps are drawn

`ChessPieceBitmaps.h` holds one `chesspieces::Set` per size. A `Glyph` has an
offset inside the square, a size, and three planes in the format
`GfxRenderer::drawGlyphBitmap()` reads: 1 bit per pixel, MSB first, rows
packed with no padding, so widths need not be multiples of 8.

```cpp
const auto& g = chesspieces::kSet64.glyphs[kind - CC_PAWN];
const glyphBitmap::Frame frame{x + g.left, y + g.top, 1, 0, 0, 1};
renderer.drawGlyphBitmap(g.halo, g.width, g.height, frame, false, mode, false);          // white
renderer.drawGlyphBitmap(black ? g.black : g.white, g.width, g.height, frame, false, mode, true);  // ink
```

- Two passes per piece: the halo in white, then the ink in black. The empty
  bits are transparent, so the same bitmap works on any square background.
- `drawGlyphBitmap` resolves orientation and clipping once per glyph. The
  same data draws in portrait, in landscape, and (with a rotated `Frame`) as
  the toppled king in the game-over dialog.
- `ChessActivity::drawPiece` takes the largest set that fits the square and
  centres it. On the X4 the 56 px set sits in a 58 px square.
- Cost: 9,843 bytes of `static const` data in flash, plus about 300 bytes of
  tables. No heap and no RAM copy.
- Checked on the host: the real `glyphBitmap::draw` renders the start position
  from the header with 0 pixel differences from the mockup renderer.

The generator needs only the Python standard library. It reads its own small
SVG subset; arcs and transforms raise an error. It runs clang-format (CI
version 21, also in `.venv`) on the header, so `bin/clang-format-fix` leaves
the file unchanged.

## 3. Layout

The UI renders in portrait like the home screen: 528×792 on the X3. All
numbers are pixels.

### Game screen, X3 portrait (Lyra metrics)

| y | Height | Content |
| --- | --- | --- |
| 0–10 | 10 | `metrics.topPadding` |
| 10–56 | 46 | header: the short band home uses (`homeTopPadding`); title, move number, battery, 3 px rule |
| 60–104 | 44 | opponent bar: side token, name, level, captured pieces, state pill |
| 106–108 | 2 | board frame |
| 108–620 | 512 | board, 8 × **64 px** squares, x 8–520 |
| 620–622 | 2 | board frame |
| 624–668 | 44 | player bar: token, name, captured pieces, material `+n`, turn pill |
| 672–752 | 80 | move strip: 2 lines × 34 px of figurine notation, newest move in a black pill |
| 752–792 | 40 | button hints (`GUI.drawButtonHints`) |

Gaps are 4 px. The side margin is 8 px and holds the frame. Square size is
`min(width − 16, height − boardTop − 160) / 8`, where 160 = gap + bar + gap +
68 px minimum strip + 40 px hints. That gives 64 on the X3.

- **Inside a 64 px square:** rank digits at (4, 1) and file letters right
  aligned at (60, 45), both in `SMALL_FONT_ID` with a 1 px white ring. Squares
  under 48 px skip coordinates.
- **Bars:** side token 18×18 at x 14; name at x 42 (`UI_12`, bold for the
  side to move); captured pieces as 24 px figurines, same kinds overlapped by
  11 px; pills right aligned at x W − 12, 28 px tall, `UI_10` bold.
- **Move strip:** wraps only before a move number, so a number never ends a
  line. When the list is longer than two lines, it starts at the earliest move
  number that still fits.
- **Notation setting:**
  - *Figurines* (default) draw the 24 px piece before the square, so move text
    needs no translation. The newest move turns the figurine into a white
    silhouette on its black pill.
  - *Letters* spell the piece with the UI language's letters through i18n:
    `Кр Ф Л С К` in Russian, `K Q R B N` in English. Pawn moves have no
    letter in either mode.
  - Measuring and wrapping are the same in both modes; only the token width
    differs.
- **Opening names:** not in the first version. A useful ECO table costs tens
  of KB of flash and needs translating. The header shows the move number
  instead. The PGN viewer can show the file's `Opening`/`ECO` tags when they
  are present, at no flash cost. A short table (about 100 common openings) can
  come later if it is missed.
- **Other themes:** RoundedRaff's band ends at 64, which leaves a 72 px strip.
  The board stays 64 px.
- **X4 (480×800):** the same code gives 58 px squares with the 56 px set. The
  strip grows to about 136 px (four lines).
- **Touch boards:** `buttonHintsHeight` is 0, so the strip grows.

### Landscape (792×528)

Board 512×512 at (8, 8), left. The side panel is x 530–742, 212 px wide: a
title rule, the opponent block, a two-column move table (32 px rows, last 8
moves), then the player block and turn pill. The theme draws the button hints
in portrait along the physical button edge, which is the right side here. So
the panel stops 40 px short of it. The arrows must use
`Button::ScreenLeft/Right/Up/Down` so they follow the rotation (see
`MappedInputManager::mapScreenDirection`).

### Hub (full Lyra header, 84 px)

- Resume card, 488×224, selected by default: a 192 px mini board (24 px
  squares with the same hatch and last-move ring), then the title, side and
  move, opponent, material, and a "Ваш ход" pill.
- Four menu rows (64 + 8 px, the `drawButtonMenu` geometry), each with a
  24 px figurine icon and a right aligned `SMALL` subtitle:
  - new game
  - puzzles
  - PGN
  - settings
- A stats line above the hints.

## 4. Board states and drawing order

Per square, back to front:

1. Background: white or hatch.
2. Last-move ring: a 6 px (s/10) 50% ring on both squares of the last move.
3. Check glow on the king's square.
4. Capture marks: four solid corner triangles with 14 px legs (7/32 of s).
5. The piece (halo, then ink).
6. Quiet-move dot: a 16 px disc with a 2 px white ring, on empty targets.
7. Selected piece: a 4 px black frame plus a 1 px white inner line.
8. Cursor: four corner brackets, arm 16 px (s/4), 4 px thick, with a 1 px
   white edge.
9. Coordinates, last.

The cursor (corners) and the selection (full frame) differ in shape, not
only in weight. The cursor still reads when it sits on the selected square or
on a capture target.

## 5. Move input on the X3 buttons

The X3 has four front buttons under the screen and two side buttons:

| Button | Position | Role in a game |
| --- | --- | --- |
| Back (btn1) | front, far left | cancel / game menu |
| Confirm (btn2) | front, left | pick up / put down / choose |
| Left (btn3) | front, right | cursor left |
| Right (btn4) | front, far right | cursor right |
| Up | left edge (y ≈ 155–233) | cursor up (toward the opponent) |
| Down | right edge (y ≈ 155–233) | cursor down |

Use the logical buttons through `mappedInput`, and label the hints with
`mapLabels()`, so the user's front-button remap keeps working. Directions are
in screen space. When the board is flipped (you play Black), Up still moves
toward the opponent. Planned: tiny arrows in the 8 px side margins next to
the edge buttons, to remind that Up/Down move the cursor. The theme's own
side-button hints have no fill, so they would cover the corner squares.

### Smart cursor

On e-ink every step costs a refresh, so the arrows do not step one square.
They jump to the nearest *useful* square in that direction. This is the
spatial navigation that TV and console UIs use. It is the only cursor mode;
there is no one-square cursor setting.

- Candidates in a direction are the squares whose offset along that direction
  is greater than 0. Score = along + 2 × across; lowest wins. Ties go to the
  smaller "across", then to file and rank order.
- With no candidate in that direction, the cursor wraps: it searches again as
  if the board repeats.
- **Picking a piece:** candidates are all your own pieces. Empty squares and
  the opponent's pieces are skipped. A piece without a legal move can hold the
  cursor, so the cursor path does not change with the position. Confirm on it
  does not pick it up; the strip says "У этой фигуры нет ходов" instead.
- **Picking a target:** candidates are the legal targets of the selected
  piece.
- A future position-setup mode would need a one-square cursor. It is not part
  of play.

### Flow, step by step

1. **Your turn.** The cursor starts on the piece you moved last, or on the
   king's pawn in the first move. Hints: `Меню · Выбрать · Влево · Вправо`.
2. **Move the cursor** with Left, Right, Up and Down. Corner brackets mark the
   square.
3. **Confirm** picks up the piece: frame on its square, dots and capture
   corners on its targets. The cursor jumps to the nearest target in the
   forward direction. Hints: `Отмена · Сходить · Влево · Вправо`. Planned:
   the bottom pill names the move ("Конь e5–f7"), and the strip says what it
   does.
4. **Arrows** jump between targets. **Confirm** plays the move; **Back** puts
   the piece down (back to step 2, cursor on that piece).
5. **Promotion.** If the move promotes, a picker column (Q, N, R, B) grows
   from the promotion square toward the centre, with a solid shadow.
   - Up/Down or Left/Right move between choices; Confirm takes one.
   - Back cancels the move (back to step 4).
   - Hints: `Отмена · Выбрать · Вверх · Вниз`.
6. **Engine turn.** The opponent pill shows "думает…". Input waits, except
   Back, which opens the menu. The reply appears with its last-move ring, and
   the cursor goes back to your last piece. If the engine cannot start (out
   of memory), the strip says so and the board stays locked; take back or a
   new game tries again.
7. **Game menu** (Back with nothing selected):
   - against the computer and between two players: continue, flip board,
     take back, new game, save and exit;
   - in training: hint, take back, redo move, flip board, new game, save and
     exit (Back continues).

   Offering a draw and resigning are planned. A long press of Back (1 s)
   leaves the game at once, to the home screen until the hub exists. The game
   is saved after every move.
8. **Game over.** A card over the board (3 px frame, hard shadow, 8 px
   radius):
   - the winner's king standing and the loser's king lying down
   - the result in Noto Sans 18 Bold
   - the reason and the move count
   - choices: new game, take back, flip board, save and exit. A rematch with
     the other colour and review in the PGN viewer are planned.

   A game that reaches 1024 plies, all the core can store, ends as a draw
   (`CC_MOVE_LIMIT`).

### Kinds of game

The new-game card picks one: against the computer, training, or two players.

- **Training** is a game against the computer with the engine's help.
  - The first line of the move strip shows a bar and the score of the
    current position from White's side ("+0.8", "#3" for a mate in three).
    A full-strength analysis runs on every turn of the player.
  - The menu starts with the tools: hint, take back, redo move.
  - A hint draws an arrow from the piece to its square, names the move in
    the strip and puts the cursor on the piece.
  - Take back removes the computer's reply too. Redo puts back what was
    taken back, until a different move is played.
- **Two players** share the device. With "auto-flip board" on (the default)
  the side to move sits at the bottom, and each turn of the board is a
  `HALF_REFRESH`.

### Puzzles and PGN

- **Puzzles** use the same board input.
  - The top bar states the task and has three progress dots: done, current,
    to go.
  - After each move, the bottom bar shows ✓ "Верно! Продолжайте" or ✗ "Не
    так", plus the streak pill.
  - The game menu becomes: hint (marks the piece to move), show the solution,
    next puzzle.
  - The theme is hidden until the puzzle is solved.
- **PGN viewer** has no cursor.
  - Left/Right step one move back or forward. Up/Down jump to the start or
    the end.
  - Confirm toggles auto-play, one move every few seconds, each with a fast
    refresh.
  - Back exits.
  - A 3 px progress bar on the strip rule shows the position in the game. The
    comment goes on the second strip line.

## 6. E-ink refresh

There is no windowed update: `displayBuffer()` sends the whole frame. With
`FAST_REFRESH` the panel only drives the pixels that changed, so the cost and
the ghosting stay where the change is. So the simplest correct approach is
also the right one: re-render the whole screen into the framebuffer, then
pick the refresh mode by event. A full re-render is about 33k hatch pixels
plus 32 glyph blits, tens of milliseconds on the C3. That is small next to
the waveform.

| Event | Refresh |
| --- | --- |
| Cursor move, select, deselect, target jump | `FAST_REFRESH` |
| Your move, engine "thinking" pill, engine reply | `FAST_REFRESH` |
| Promotion picker open or close | `FAST_REFRESH` (small area) |
| Game menu, new-game card or result card open or close | `FAST_REFRESH` |
| Every N moves (N = `SETTINGS.getRefreshFrequency()`, like reader pages) | `HALF_REFRESH` |
| Enter the game, new game, board flip (also the automatic one between two players), take back, redo | `HALF_REFRESH` |
| Leaving the chess app | whatever the next screen does (home: `HALF` on first render) |

- The move counter works like `ReaderUtils::displayWithRefreshCycle`: count
  fast refreshes and promote one to `HALF_REFRESH` when it reaches N.
- Keep large black areas still. Pills are small, and the black "now" pill
  moves between the two bars only on a turn change.
- No grayscale pass in the game. The X3 gray overlay needs a base waveform
  plus two plane writes, which is slow and flashes. It would also turn every
  cursor step into a multi-pass update. Hatch and dither give enough tones.
- No running seconds. Timed games update the clocks on each move. Above one
  minute they show minutes only, and they tick at most once a minute with a
  fast refresh.
- The engine searches in its own task, so the render task and input keep
  running. The task is created on the first search and deleted when the game
  screen closes; each level has a time budget. A reply faster than 0.5 s
  waits, so the "думает…" pill and the move do not refresh back to back.

## 7. Mockups

`render_mockups.py` draws with:

- the firmware's built-in font bitmaps, parsed from
  `lib/EpdFont/builtinFonts/*.h`, including the DEFLATE-compressed Noto groups;
- the firmware dither patterns;
- the generated piece bitmaps.

So a PNG shows only what the panel can show. Known differences: no kerning,
so text can be 1–2 px wider; and the header and hints are re-drawn
approximations of the Lyra components.

| File | Screen |
| --- | --- |
| `1_hub.png` | Chess hub with the resume card |
| `2_game.png` | Game, your turn, cursor on a piece |
| `2b_game_letters.png` | Same game with the notation setting on letters |
| `3_move_select.png` | Piece selected, targets and captures, cursor on a capture |
| `4_promotion.png` | Promotion picker |
| `5_game_over.png` | Checkmate dialog |
| `6_puzzle.png` | Puzzle after a correct move |
| `7_pgn.png` | PGN viewer (Morphy, Paris 1858, after 16.Qb8+) |
| `8_game_landscape.png` | Game in landscape |

## 8. Strings

The built screens use the `STR_CHESS_*` keys in
`lib/I18n/translations/english.yaml` and `russian.yaml`; other languages fall
back to English. The planned features bring their own: piece letters for the
notation setting (Russian: Кр, Ф, Л, С, К), puzzle feedback, hub rows, draw
and resign.
