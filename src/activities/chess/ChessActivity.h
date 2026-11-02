#pragma once

#ifdef CROSSCHESS_ENABLED

#include <HalDisplay.h>
#include <crosschess.h>

#include <array>
#include <cstdint>
#include <vector>

#include "activities/Activity.h"

struct ChessEngine;  // the engine's worker task; ChessActivity.cpp

// A game of chess on the buttons, backed by the Rust core in chess/. Three
// kinds of game: against the engine; training against it, with the
// evaluation on screen, hints and moves taken back and replayed; and two
// players on one device, the board turning to whoever moves. The arrows jump
// between useful squares, Confirm picks up and puts down, Back opens the game
// menu. The engine thinks in a task of its own so the screen stays
// responsive. The game is saved to the SD card after every move.
// Design: design/README.md.
class ChessActivity final : public Activity {
 public:
  explicit ChessActivity(GfxRenderer& renderer, MappedInputManager& mappedInput)
      : Activity("Chess", renderer, mappedInput) {}
  ~ChessActivity() override;
  void onEnter() override;
  void onExit() override;
  void loop() override;
  void render(RenderLock&&) override;
  // Searching needs the full clock speed, which the main loop keeps while an
  // activity holds off sleep.
  bool preventAutoSleep() override { return searching; }

 private:
  enum class Mode : uint8_t { PickPiece, PickTarget, Promotion, Menu, NewGame };
  enum class MenuItem : uint8_t { Continue, Hint, NewGame, TakeBack, Redo, Flip, Exit };
  enum class GameKind : uint8_t { Engine, Training, TwoPlayers };
  enum class SideChoice : uint8_t { White, Black, Random };
  using San = std::array<char, 8>;

  // Training: the engine's view of the current position.
  struct Analysis {
    bool valid = false;
    int whiteScore = 0;  // centipawns, from White's side
    uint16_t best = 0;
    San bestSan{};
  };

  // The game and what the screen needs of it, refreshed after every change.
  cc_game game{};
  cc_status status{};
  uint8_t squares[64] = {};
  uint16_t legalMoves[CC_MAX_MOVES] = {};
  int legalCount = 0;
  uint16_t lastMove = 0;
  std::vector<San> sanMoves;        // one per ply, for the move strip
  uint8_t captured[2][5] = {};      // [side that took][pawn..queen]
  int material = 0;                 // white minus black, in pawns
  std::vector<uint16_t> redoMoves;  // moves taken back, the last one first to replay

  // Who plays this game; saved with it.
  GameKind kind = GameKind::Engine;
  bool playerWhite = true;
  int level = 3;
  bool autoFlip = true;  // two players: the side to move sits at the bottom

  // The new-game card, before Start.
  GameKind setupKind = GameKind::Engine;
  SideChoice setupSide = SideChoice::White;
  int setupLevel = 3;
  bool setupAutoFlip = true;
  int setupRow = 0;
  Mode setupReturn = Mode::Menu;  // where Back from the card goes

  ChessEngine* engine = nullptr;  // created on the first search, freed on exit
  bool searching = false;         // a search was started and its result not yet taken
  bool thinking = false;          // the opponent is searching; read by the render task
  int engineFailedAt = -1;        // ply where the engine could not start
  Analysis analysis;
  bool hintRequested = false;  // show the best move once the analysis has it

  Mode mode = Mode::PickPiece;
  int cursor = 0;
  int selected = -1;  // square of the piece picked up
  int promotionTo = -1;
  int promotionChoice = 0;
  int menuChoice = 0;
  bool flipped = false;
  bool showNoMoves = false;
  bool leaving = false;

  HalDisplay::RefreshMode nextRefresh = HalDisplay::HALF_REFRESH;
  int movesUntilHalfRefresh = 0;

  // Game
  void newGame();
  bool load();
  void save() const;
  void refreshPosition();
  void play(uint16_t move, bool replaying = false);
  bool undo();
  void takeBack();
  void redo();
  bool isOver() const { return status.outcome != CC_ONGOING; }
  bool againstEngine() const { return kind != GameKind::TwoPlayers; }
  bool engineToMove() const { return againstEngine() && status.white_to_move != playerWhite; }
  // The player waits while it is the engine's turn, also when it could not start.
  bool boardLocked() const { return engineToMove() && !isOver(); }
  bool hintShown() const { return hintRequested && analysis.valid && analysis.best != 0; }
  bool ownPiece(int square) const;
  int defaultCursor() const;
  uint64_t targetsOf(int from) const;
  uint16_t findMove(int from, int to, int promotion) const;

  // Engine
  bool ensureEngine();
  void startSearch(bool analyse);
  void cancelSearch();
  void shutDownEngine();
  void pollEngine();
  void scheduleEngine();

  // Input
  void handleBoardInput();
  void handleMenuInput();
  void handleSetupInput();
  void handleArrow(int dx, int dy);
  void handleConfirm();
  void handleBack();
  void leave();
  int menuCount() const { return isOver() ? 4 : kind == GameKind::Training ? 6 : 5; }
  MenuItem menuItem(int index) const;
  void runMenuItem(MenuItem item);
  void openSetup(Mode returnTo);
  void startNewGame();
  int setupRowCount() const { return setupKind == GameKind::TwoPlayers ? 3 : 4; }
  int promotionStep() const;

  // Drawing
  int screenCol(int square) const;
  int screenRow(int square) const;
  void drawBoard(int x, int y, int squareSize) const;
  void drawSquare(int x, int y, int squareSize, int square, uint64_t quiet, uint64_t captures) const;
  void drawPiece(int x, int y, int squareSize, uint8_t piece) const;
  void drawHintArrow(int boardX, int boardY, int squareSize) const;
  void drawBar(int y, bool white) const;
  void drawTrainingLine(int y) const;
  void drawMoveStrip(int y, int height) const;
  void drawPromotionPicker(int boardX, int boardY, int squareSize) const;
  void drawMenu(int boardX, int boardY, int squareSize) const;
  void drawSetup(int boardX, int boardY, int squareSize) const;
  int drawSan(int x, int y, const char* san, bool inverted, bool draw) const;
};

#endif  // CROSSCHESS_ENABLED
