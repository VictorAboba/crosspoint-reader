// C API of the CrossChess Rust core (chess/ffi). Keep in sync with chess/ffi/src/lib.rs.
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Piece codes: 0 is an empty square; the kind is in bits 0-2 and bit 3 is set for black.
enum {
  CC_PAWN = 1,
  CC_KNIGHT = 2,
  CC_BISHOP = 3,
  CC_ROOK = 4,
  CC_QUEEN = 5,
  CC_KING = 6,
  CC_BLACK = 8,
};

// Moves are 16 bits: from square in bits 0-5, to square in bits 6-11, kind in bits 12-15.
// Squares count from a1 = 0 to h8 = 63. 0 is never a legal move, so it means "none".
enum {
  CC_MOVE_NORMAL = 0,
  CC_MOVE_DOUBLE_PUSH = 1,
  CC_MOVE_CASTLE_KINGSIDE = 2,
  CC_MOVE_CASTLE_QUEENSIDE = 3,
  CC_MOVE_EN_PASSANT = 4,
  CC_MOVE_PROMOTE_KNIGHT = 5,
  CC_MOVE_PROMOTE_BISHOP = 6,
  CC_MOVE_PROMOTE_ROOK = 7,
  CC_MOVE_PROMOTE_QUEEN = 8,
};

static inline uint8_t cc_move_from(uint16_t move) { return move & 63; }
static inline uint8_t cc_move_to(uint16_t move) { return (move >> 6) & 63; }
static inline uint8_t cc_move_kind(uint16_t move) { return move >> 12; }
// The piece kind a promotion makes (CC_KNIGHT to CC_QUEEN), or 0.
static inline uint8_t cc_move_promotion(uint16_t move) {
  const uint8_t kind = cc_move_kind(move);
  return kind >= CC_MOVE_PROMOTE_KNIGHT && kind <= CC_MOVE_PROMOTE_QUEEN ? kind - 3 : 0;
}

// Most legal moves a position can have, and most moves a game can hold.
#define CC_MAX_MOVES 256
#define CC_MAX_PLIES 1024

// How a game ended.
enum {
  CC_ONGOING = 0,
  CC_CHECKMATE = 1,  // the side to move is mated
  CC_STALEMATE = 2,
  CC_FIFTY_MOVES = 3,
  CC_INSUFFICIENT_MATERIAL = 4,
  CC_THREEFOLD_REPETITION = 5,
  CC_MOVE_LIMIT = 6,  // a draw: the game holds CC_MAX_PLIES moves, all it can store
};

// A game: start position, moves and current position. Fixed size, no heap; about 2.3 KB.
// Set it up with cc_game_init before any other call. It is plain data: copying
// it by assignment gives an independent game.
#define CC_GAME_WORDS 600
typedef struct cc_game {
  uint32_t opaque[CC_GAME_WORDS];
} cc_game;

typedef struct cc_status {
  uint16_t plies;            // moves played since the start position
  uint16_t fullmove_number;  // of the current position, as in FEN
  bool white_to_move;
  bool in_check;
  uint8_t outcome;  // CC_ONGOING or how the game ended
} cc_status;

// Scratch space of the engine, about 9.4 KB. Zero-filled storage is ready to
// use (allocate it with `new cc_engine()`, or memset it to 0).
#define CC_ENGINE_WORDS 2400
typedef struct cc_engine {
  uint32_t opaque[CC_ENGINE_WORDS];
} cc_engine;

// A mate in n plies scores CC_MATE - n.
#define CC_MATE 30000

typedef struct cc_search_limits {
  uint8_t max_depth;  // iterations of iterative deepening, 1 and up
  uint16_t noise;     // evaluation noise in centipawns; 0 plays the best moves
  uint32_t seed;      // mixed into the noise; a new one per search varies the play
  // Called every 1024 nodes with the node count; return true to stop. The
  // first iteration always finishes, so a move is always found. May be NULL.
  bool (*should_stop)(void* context, uint32_t nodes);
  void* context;
} cc_search_limits;

typedef struct cc_search_result {
  uint16_t move;   // best move, 0 if the side to move has none
  uint8_t depth;   // last fully searched depth
  int32_t score;   // centipawns for the side to move
  uint32_t nodes;  // positions searched
} cc_search_result;

// Core version, e.g. "0.1.0". Static string.
const char* cc_version(void);

// FEN of the standard starting position. Static string.
const char* cc_startpos_fen(void);

// Parses `fen` and writes the piece code of every square (a1 = 0, h8 = 63) to `out`.
// Returns false and leaves `out` untouched if `fen` is not valid.
bool cc_fen_to_squares(const char* fen, uint8_t out[64]);

// Starts a game from `fen`, or from the standard position when `fen` is NULL.
// Returns false and leaves `game` untouched if `fen` is not valid.
bool cc_game_init(cc_game* game, const char* fen);

// Piece code of every square of the current position.
void cc_game_squares(const cc_game* game, uint8_t out[64]);

void cc_game_status(const cc_game* game, cc_status* out);

// Writes the legal moves of the current position and returns how many there are.
size_t cc_game_legal_moves(const cc_game* game, uint16_t out[CC_MAX_MOVES]);

// Plays `move` if it is legal. Returns false and changes nothing otherwise.
bool cc_game_play(cc_game* game, uint16_t move);

// Takes back the last move. Returns false at the start of the game.
bool cc_game_undo(cc_game* game);

// The move played at `ply` (0 is the first), or 0 past the end.
uint16_t cc_game_move(const cc_game* game, size_t ply);

// The legal move written in UCI ("e2e4", "e7e8q"), or 0.
uint16_t cc_game_parse_uci(const cc_game* game, const char* uci);

// SAN of a legal move in the current position ("Nf3", "exd8=Q+", "O-O"), with
// English piece letters. Returns the length, or 0 if the move is not legal or
// `out` is too small. 8 bytes hold any move.
size_t cc_game_san(const cc_game* game, uint16_t move, char* out, size_t size);

// FEN of the start position (`start`) or of the current one. Returns the
// length, or 0 if `out` is too small. 96 bytes hold any FEN.
size_t cc_game_fen(const cc_game* game, bool start, char* out, size_t size);

// Searches the current position of `game` and writes the best move to `out`.
// Returns false when the side to move has no legal move. Runs until the depth
// limit or until `should_stop` says so; run it off the UI task.
bool cc_engine_search(cc_engine* engine, const cc_game* game, const cc_search_limits* limits, cc_search_result* out);

// UCI of `move`. Returns the length (4 or 5).
size_t cc_move_uci(uint16_t move, char out[6]);

#ifdef __cplusplus
}

// The layouts chess/ffi/src/lib.rs asserts too.
static_assert(sizeof(cc_status) == 8 && offsetof(cc_status, outcome) == 6, "cc_status layout");
static_assert(sizeof(cc_search_result) == 12 && offsetof(cc_search_result, score) == 4 &&
                  offsetof(cc_search_result, nodes) == 8,
              "cc_search_result layout");
static_assert(offsetof(cc_search_limits, seed) == 4 && offsetof(cc_search_limits, should_stop) == 8 &&
                  offsetof(cc_search_limits, context) == 8 + sizeof(void*),
              "cc_search_limits layout");
#endif
