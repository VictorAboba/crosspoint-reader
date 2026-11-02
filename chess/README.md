# CrossChess core

Chess rules and engine in Rust, linked into the firmware as a static library.

```
chess/
  core/      crosschess-core: board, rules, engine (no_std, no heap)
  ffi/       crosschess-ffi: C API over the core, built as a staticlib
  include/   crosschess.h: the C header for that API (keep in sync with ffi/)
```

The core has the board and FEN (`board.rs`), legal moves and results
(`movegen.rs`), a game with take-back and threefold repetition (`game.rs`),
SAN (`san.rs`), and the engine: position keys (`hash.rs`), evaluation
(`eval.rs`) and an alpha-beta search (`engine.rs`). The C API wraps a game in
a fixed-size `cc_game` and the engine's scratch space in `cc_engine`, so the
core needs no heap.

`cargo test --release -p crosschess-core -- --ignored --nocapture engine_speed`
prints the search speed per depth.

The firmware UI lives in `src/activities/chess/`.

## Using it

- Chess is a row of the home screen's menu list; the Cover Grid home layout
  has no entry for it yet.
- Back opens the game menu; holding Back for 1 s leaves the game. The game is
  saved after every move to `/crosschess/game.txt` on the SD card.
- ESP32-C3 builds also carry a boot guard (`src/crosschess/BootGuard.h`) for
  devices that can only be updated over Wi-Fi or from the SD card:
  - If a new build fails to start three times in a row, the device switches
    back to the firmware it replaced. A start counts as good after 10 s of
    running, or when the device goes to sleep. Until then the chip's watchdog
    resets a start that freezes for a minute, and that start counts as failed.
  - With the device off and USB unplugged, hold Side Down (right edge on the
    X3), then press and hold Power: it starts the previous firmware and keeps
    starting it until the newer build is installed again. With USB power the
    device starts without checking the buttons.
  - On its first start, a new build copies the firmware it replaced to
    `/crosschess/backup` on the SD card, with a README. The screen stays as it
    was for several seconds while it copies. This happens after every update,
    unless that firmware is already on the card. A file from there can be
    installed through Recovery mode (Side Up + Power). Each copy takes about
    6 MB; old ones can be deleted.

## Build

`scripts/build_chess_core.py` runs `cargo build` before every PlatformIO build
and links the library. It only does so for MCUs with a Rust target (currently
ESP32-C3); it then defines `CROSSCHESS_ENABLED`. Other boards build without chess.

It needs Rust 1.85 or newer (the crates use edition 2024). One-time setup:

```sh
rustup target add riscv32imc-unknown-none-elf
```

The script then runs the toolchain's `objcopy` so that only the `cc_*`
functions stay global in the library the firmware links. The library also
carries Rust's `compiler_builtins`, whose copies of C maths and libgcc helpers
would otherwise replace the firmware's own.

## Tests

Run on the host:

```sh
cargo test --workspace
```

The move generator is checked with perft against published node counts. The
deeper runs (tens of millions of nodes) are ignored by default:

```sh
cargo test --release -p crosschess-core -- --ignored
```

On Windows without the MSVC Build Tools, use the GNU toolchain:

```sh
rustup toolchain install stable-x86_64-pc-windows-gnu --profile minimal
cargo +stable-x86_64-pc-windows-gnu test --workspace
```
