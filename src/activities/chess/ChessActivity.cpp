#include "ChessActivity.h"

#ifdef CROSSCHESS_ENABLED

#include <Arduino.h>
#include <GfxRenderer.h>
#include <GlyphBitmap.h>
#include <HalStorage.h>
#include <I18n.h>
#include <esp_random.h>
#include <freertos/FreeRTOS.h>
#include <freertos/task.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <new>
#include <string>

#include "ChessCursor.h"
#include "ChessPieceBitmaps.h"
#include "CrossPointSettings.h"
#include "components/UITheme.h"
#include "fontIds.h"

namespace {
// Game screen geometry, see design/README.md "Layout".
constexpr int kBarHeight = 44;            // player bar above and below the board
constexpr int kGap = 4;                   // between header, bars, board and move strip
constexpr int kMinStripHeight = 68;       // two move-list lines under the bottom bar
constexpr int kStripLineHeight = 34;      // one line of the move strip
constexpr int kBoardMargin = 8;           // side margin; holds the board frame
constexpr int kFrameWidth = 2;            // board frame, drawn in the margin
constexpr int kHatchPeriod = 4;           // dark squares: 1 px diagonal every 4 px
constexpr int kMinCoordinateSquare = 48;  // smaller squares skip a-h / 1-8
constexpr int kTokenSize = 18;            // side token in the player bars
constexpr int kNameX = 42;                // player name in the bars
constexpr int kPillHeight = 28;
constexpr int kPillInset = 12;     // pills from the screen edge, text inside pills
constexpr int kFrameThick = 4;     // selection frame and cursor brackets
constexpr int kCardRadius = 8;     // menu card and promotion picker
constexpr int kCardShadow = 6;     // solid shadow under cards
constexpr int kFigurineStep = 13;  // same captured kinds overlap by 11 of 24 px

constexpr unsigned long kLeaveHoldMs = 1000;  // long Back leaves the game

constexpr char kSaveDir[] = "/crosschess";
constexpr char kSavePath[] = "/crosschess/game.txt";
constexpr char kSaveTempPath[] = "/crosschess/game.tmp";
constexpr size_t kMaxSaveBytes = 16 * 1024;  // a full game is about 6 KB

// Promotion picker order, from the promotion square toward the centre.
constexpr uint8_t kPromotionKinds[4] = {CC_QUEEN, CC_KNIGHT, CC_ROOK, CC_BISHOP};
// Pawn to queen, for captured pieces and the material count.
constexpr int kPieceValues[5] = {1, 3, 3, 5, 9};
constexpr int kStartCounts[5] = {8, 2, 2, 2, 1};

// 4x4 Bayer thresholds for the check glow.
constexpr uint8_t kBayer[16] = {0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5};

bool hasBit(const uint64_t bits, const int square) { return (bits >> square) & 1; }

// Hatch in screen coordinates so the lines continue across squares.
void drawHatch(const GfxRenderer& renderer, const int x, const int y, const int size) {
  for (int row = 0; row < size; row++) {
    for (int col = (kHatchPeriod - (x + y + row) % kHatchPeriod) % kHatchPeriod; col < size; col += kHatchPeriod) {
      renderer.drawPixel(x + col, y + row, true);
    }
  }
}

void fillDisc(const GfxRenderer& renderer, const int cx, const int cy, const int radius, const bool state) {
  for (int dy = -radius; dy < radius; dy++) {
    for (int dx = -radius; dx < radius; dx++) {
      // Centre the disc between pixels so even sizes stay symmetric.
      const int px = 2 * dx + 1;
      const int py = 2 * dy + 1;
      if (px * px + py * py <= 4 * radius * radius) renderer.drawPixel(cx + dx, cy + dy, state);
    }
  }
}

// Black label inside a 1 px white ring, readable on hatched squares.
void drawCoordinate(const GfxRenderer& renderer, const int x, const int y, const char* label) {
  for (int dy = -1; dy <= 1; dy++) {
    for (int dx = -1; dx <= 1; dx++) {
      if (dx != 0 || dy != 0) renderer.drawText(SMALL_FONT_ID, x + dx, y + dy, label, false);
    }
  }
  renderer.drawText(SMALL_FONT_ID, x, y, label, true);
}

// White card with a 3 px frame and a solid shadow, as the design's dialogs.
void drawCard(const GfxRenderer& renderer, const int x, const int y, const int width, const int height) {
  renderer.fillRoundedRect(x + kCardShadow, y + kCardShadow, width, height, kCardRadius, Color::Black);
  renderer.fillRoundedRect(x, y, width, height, kCardRadius, Color::White);
  renderer.drawRoundedRect(x, y, width, height, 3, kCardRadius, true);
}

// A 4 px frame with a 1 px white line inside: the picked-up piece.
void drawSelectionFrame(const GfxRenderer& renderer, const int x, const int y, const int size) {
  renderer.drawRect(x, y, size, size, kFrameThick, true);
  renderer.drawRect(x + kFrameThick, y + kFrameThick, size - 2 * kFrameThick, size - 2 * kFrameThick, 1, false);
}

// Four corner brackets with a 1 px white edge: the cursor.
void drawCursor(const GfxRenderer& renderer, const int x, const int y, const int size) {
  const int arm = size / 4;
  for (int corner = 0; corner < 4; corner++) {
    const bool right = corner & 1;
    const bool bottom = corner & 2;
    const int cornerX = right ? x + size - arm : x;
    const int cornerY = bottom ? y + size - arm : y;
    const int barY = bottom ? y + size - kFrameThick : y;
    const int barX = right ? x + size - kFrameThick : x;
    // White first, one pixel wider on the inner sides.
    renderer.fillRect(right ? cornerX - 1 : cornerX, bottom ? barY - 1 : barY, arm + 1, kFrameThick + 1, false);
    renderer.fillRect(right ? barX - 1 : barX, bottom ? cornerY - 1 : cornerY, kFrameThick + 1, arm + 1, false);
    renderer.fillRect(cornerX, barY, arm, kFrameThick, true);
    renderer.fillRect(barX, cornerY, kFrameThick, arm, true);
  }
}

// Solid triangles in the four corners: a capture target.
void drawCaptureMarks(const GfxRenderer& renderer, const int x, const int y, const int size) {
  const int leg = size * 7 / 32;
  const int right = x + size - 1;
  const int bottom = y + size - 1;
  const int corners[4][6] = {
      {x, y, x + leg, y, x, y + leg},
      {right, y, right - leg, y, right, y + leg},
      {x, bottom, x + leg, bottom, x, bottom - leg},
      {right, bottom, right - leg, bottom, right, bottom - leg},
  };
  for (const auto& c : corners) {
    const int xs[3] = {c[0], c[2], c[4]};
    const int ys[3] = {c[1], c[3], c[5]};
    renderer.fillPolygon(xs, ys, 3, true);
  }
}

// Dense ink near the centre fading out to the corners: the king in check.
void drawCheckGlow(const GfxRenderer& renderer, const int x, const int y, const int size) {
  const float reach = size * 0.75f;
  const float centre = (size - 1) / 2.0f;
  for (int row = 0; row < size; row++) {
    for (int col = 0; col < size; col++) {
      const float distance = std::hypot(col - centre, row - centre);
      const int level = static_cast<int>(16.0f * (1.0f - distance / reach));
      const int px = x + col;
      const int py = y + row;
      if (level > kBayer[(py % 4) * 4 + px % 4]) renderer.drawPixel(px, py, true);
    }
  }
}

// Largest piece set that fits the square: 64 on the X3, 56 on 480 px panels.
const chesspieces::Set& pieceSetFor(const int squareSize) {
  if (squareSize >= chesspieces::kSet64.squareSize) return chesspieces::kSet64;
  if (squareSize >= chesspieces::kSet56.squareSize) return chesspieces::kSet56;
  return chesspieces::kSet24;
}

// Draws one glyph: the white halo, then the ink. `inverted` draws the black
// piece's solid shape in white, for a figurine on a black pill.
void drawGlyph(const GfxRenderer& renderer, const chesspieces::Glyph& glyph, const glyphBitmap::Frame& frame,
               const bool black, const bool inverted) {
  const auto mode = renderer.getRenderMode();
  if (inverted) {
    renderer.drawGlyphBitmap(glyph.black, glyph.width, glyph.height, frame, false, mode, false);
    return;
  }
  renderer.drawGlyphBitmap(glyph.halo, glyph.width, glyph.height, frame, false, mode, false);
  renderer.drawGlyphBitmap(black ? glyph.black : glyph.white, glyph.width, glyph.height, frame, false, mode, true);
}

void drawFigurine(const GfxRenderer& renderer, const int x, const int y, const int kind, const bool black,
                  const bool inverted = false) {
  const auto& glyph = chesspieces::kSet24.glyphs[kind - CC_PAWN];
  drawGlyph(renderer, glyph, glyphBitmap::Frame{x + glyph.left, y + glyph.top, 1, 0, 0, 1}, black, inverted);
}

// SAN uses English letters; the strip shows these as figurines.
int pieceKindForLetter(const char c) {
  switch (c) {
    case 'N':
      return CC_KNIGHT;
    case 'B':
      return CC_BISHOP;
    case 'R':
      return CC_ROOK;
    case 'Q':
      return CC_QUEEN;
    case 'K':
      return CC_KING;
    default:
      return 0;
  }
}

StrId outcomeReason(const uint8_t outcome) {
  switch (outcome) {
    case CC_CHECKMATE:
      return StrId::STR_CHESS_CHECKMATE;
    case CC_STALEMATE:
      return StrId::STR_CHESS_STALEMATE;
    case CC_FIFTY_MOVES:
      return StrId::STR_CHESS_FIFTY_MOVES;
    case CC_INSUFFICIENT_MATERIAL:
      return StrId::STR_CHESS_INSUFFICIENT_MATERIAL;
    case CC_MOVE_LIMIT:
      return StrId::STR_CHESS_MOVE_LIMIT;
    default:
      return StrId::STR_CHESS_REPETITION;
  }
}
}  // namespace

// --- Engine task -----------------------------------------------------------

// The engine worker: one task and its scratch space for the life of the chess
// screen, created on the first search. The task sleeps until notified,
// searches `game`, then clears `busy`; it touches nothing else.
struct ChessEngine {
  cc_game game;       // a copy, so the activity can change its own game meanwhile
  cc_engine scratch;  // zero-filled by `new ChessEngine()`, as the core expects
  cc_search_limits limits;
  cc_search_result result;
  bool analysis;   // training: evaluate the player's position instead of moving
  uint16_t plies;  // the position searched, to tell a result that came too late
  uint16_t lastMove;
  unsigned long startedMs;
  unsigned long budgetMs;
  unsigned long lastYieldMs;
  uint32_t stackLeft;
  TaskHandle_t task;
  std::atomic<bool> cancel{false};
  std::atomic<bool> busy{false};
};

namespace {
// Search depth, evaluation noise and thinking time per level. Low levels look
// one or two plies ahead through a noisy evaluation; the top ones search as
// deep as their time allows.
struct Level {
  uint8_t depth;
  uint16_t noise;
  uint16_t thinkMs;
};
constexpr Level kLevels[] = {
    {1, 350, 800}, {1, 200, 800}, {2, 120, 1500}, {3, 70, 2500},
    {3, 35, 3500}, {4, 15, 5000}, {5, 0, 7000},   {8, 0, 10000},
};
constexpr int kLevelCount = sizeof(kLevels) / sizeof(kLevels[0]);

// Training analyses the player's position at full strength, as deep as this
// time allows.
constexpr uint8_t kAnalysisDepth = 12;
constexpr unsigned long kAnalysisMs = 3000;

// The search keeps its move lists in cc_engine, so a ply of recursion costs
// a board copy and a few locals: about 9 KB at the 32-ply limit. The log line
// after each search reports what was left unused.
constexpr uint32_t kEngineStackBytes = 16 * 1024;
// A faster reply waits this long, so "thinking" and the move do not refresh
// the panel back to back.
constexpr unsigned long kMinThinkMs = 500;
// The search yields this often so the idle task can feed the watchdog and
// the UI tasks get the CPU.
constexpr unsigned long kYieldEveryMs = 20;

bool engineShouldStop(void* context, uint32_t /*nodes*/) {
  auto* engine = static_cast<ChessEngine*>(context);
  const unsigned long now = millis();
  if (now - engine->lastYieldMs >= kYieldEveryMs) {
    vTaskDelay(1);
    engine->lastYieldMs = millis();
  }
  return engine->cancel.load(std::memory_order_relaxed) || now - engine->startedMs >= engine->budgetMs;
}

void engineTask(void* parameter) {
  auto* engine = static_cast<ChessEngine*>(parameter);
  for (;;) {
    ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
    cc_engine_search(&engine->scratch, &engine->game, &engine->limits, &engine->result);
    engine->stackLeft = uxTaskGetStackHighWaterMark(nullptr);
    engine->busy.store(false, std::memory_order_release);
  }
}

// Stops a running search: the core checks every 1024 positions, after its
// first iteration. Takes no lock.
void waitForIdle(ChessEngine& engine) {
  engine.cancel.store(true, std::memory_order_relaxed);
  while (engine.busy.load(std::memory_order_acquire)) {
    vTaskDelay(1);
  }
}

// "+0.8", "-1.5", "0.0", or "#3" / "#-3" for a mate in three; from White's side.
void formatScore(const int whiteScore, char* out, const size_t size) {
  const int magnitude = std::abs(whiteScore);
  if (magnitude >= CC_MATE - 100) {
    snprintf(out, size, whiteScore > 0 ? "#%d" : "#-%d", (CC_MATE - magnitude + 1) / 2);
    return;
  }
  const int tenths = (magnitude + 5) / 10;
  const char* sign = tenths == 0 ? "" : whiteScore > 0 ? "+" : "-";
  snprintf(out, size, "%s%d.%d", sign, tenths / 10, tenths % 10);
}
}  // namespace

// --- Game ------------------------------------------------------------------

ChessActivity::~ChessActivity() { shutDownEngine(); }

void ChessActivity::onEnter() {
  Activity::onEnter();
  nextRefresh = HalDisplay::HALF_REFRESH;
  movesUntilHalfRefresh = SETTINGS.getRefreshFrequency();
  if (!load()) {
    // First visit: a game against the engine is ready behind the new-game
    // card, so Confirm starts playing.
    newGame();
    openSetup(Mode::PickPiece);
  }
  LOG_INF("CHESS", "Rust core %s, %d plies", cc_version(), static_cast<int>(status.plies));
  requestUpdate();
}

void ChessActivity::onExit() {
  shutDownEngine();
  Activity::onExit();
}

void ChessActivity::newGame() {
  cc_game_init(&game, nullptr);
  sanMoves.clear();
  redoMoves.clear();
  refreshPosition();
  mode = Mode::PickPiece;
  selected = -1;
  showNoMoves = false;
  engineFailedAt = -1;
  cursor = defaultCursor();
}

// The save file has one "key value" line each: the start position, the moves
// in UCI, the board orientation and who plays. Unknown lines are skipped.
bool ChessActivity::load() {
  std::string text;
  if (!Storage.exists(kSavePath) || !Storage.readFileToString("CHESS", kSavePath, kMaxSaveBytes, text)) return false;

  std::string fen;
  std::string moves;
  bool savedFlip = false;
  size_t lineStart = 0;
  while (lineStart < text.size()) {
    size_t lineEnd = text.find('\n', lineStart);
    if (lineEnd == std::string::npos) lineEnd = text.size();
    const std::string line = text.substr(lineStart, lineEnd - lineStart);
    lineStart = lineEnd + 1;
    const size_t space = line.find(' ');
    if (space == std::string::npos) continue;
    const std::string key = line.substr(0, space);
    const std::string value = line.substr(space + 1);
    if (key == "fen") fen = value;
    if (key == "moves") moves = value;
    if (key == "flipped") savedFlip = value == "1";
    if (key == "mode") {
      kind = value == "training" ? GameKind::Training : value == "two" ? GameKind::TwoPlayers : GameKind::Engine;
    }
    if (key == "player") playerWhite = value != "black";
    if (key == "level") level = std::clamp(std::atoi(value.c_str()), 1, kLevelCount);
    if (key == "autoflip") autoFlip = value != "0";
  }
  if (fen.empty() || !cc_game_init(&game, fen.c_str())) {
    LOG_ERR("CHESS", "Saved game has no valid start position");
    return false;
  }

  // Replay the moves, keeping the SAN of each. A move that does not parse
  // ends the replay, so a damaged file still gives the game up to there.
  sanMoves.clear();
  redoMoves.clear();
  size_t tokenStart = 0;
  while (tokenStart < moves.size()) {
    size_t tokenEnd = moves.find(' ', tokenStart);
    if (tokenEnd == std::string::npos) tokenEnd = moves.size();
    const std::string uci = moves.substr(tokenStart, tokenEnd - tokenStart);
    tokenStart = tokenEnd + 1;
    if (uci.empty()) continue;
    const uint16_t move = cc_game_parse_uci(&game, uci.c_str());
    San san{};
    if (move == 0 || cc_game_san(&game, move, san.data(), san.size()) == 0 || !cc_game_play(&game, move)) {
      LOG_ERR("CHESS", "Saved game stops at bad move %s", uci.c_str());
      break;
    }
    sanMoves.push_back(san);
  }

  flipped = savedFlip;
  refreshPosition();
  mode = isOver() ? Mode::Menu : Mode::PickPiece;
  menuChoice = 0;
  selected = -1;
  cursor = defaultCursor();
  return true;
}

void ChessActivity::save() const {
  char fen[96];
  if (cc_game_fen(&game, true, fen, sizeof(fen)) == 0) return;
  std::string text = "fen ";
  text += fen;
  text += "\nmoves";
  for (int ply = 0; ply < status.plies; ply++) {
    char uci[6];
    cc_move_uci(cc_game_move(&game, ply), uci);
    text += ' ';
    text += uci;
  }
  const char* kindName = kind == GameKind::Training ? "training" : kind == GameKind::TwoPlayers ? "two" : "engine";
  char settings[96];
  snprintf(settings, sizeof(settings), "\nflipped %d\nmode %s\nplayer %s\nlevel %d\nautoflip %d\n", flipped ? 1 : 0,
           kindName, playerWhite ? "white" : "black", level, autoFlip ? 1 : 0);
  text += settings;

  // Write a temp file and move it over the save, so a power cut mid-write
  // keeps the previous save.
  if (!Storage.ensureDirectoryExists(kSaveDir) || !Storage.writeFile(kSaveTempPath, String(text.c_str())) ||
      !Storage.replaceFile(kSaveTempPath, kSavePath)) {
    LOG_ERR("CHESS", "Could not save the game");
  }
}

void ChessActivity::refreshPosition() {
  cc_game_squares(&game, squares);
  cc_game_status(&game, &status);
  legalCount = static_cast<int>(cc_game_legal_moves(&game, legalMoves));
  lastMove = status.plies > 0 ? cc_game_move(&game, status.plies - 1) : 0;

  int counts[2][5] = {};
  for (const uint8_t piece : squares) {
    const int pieceKind = piece & 7;
    if (pieceKind >= CC_PAWN && pieceKind <= CC_QUEEN) counts[(piece & CC_BLACK) ? 1 : 0][pieceKind - CC_PAWN]++;
  }
  material = 0;
  for (int i = 0; i < 5; i++) {
    // A side "took" what the other side is missing. Promotions can push a
    // count above the start, so clamp at zero.
    captured[0][i] = static_cast<uint8_t>(std::max(0, kStartCounts[i] - counts[1][i]));
    captured[1][i] = static_cast<uint8_t>(std::max(0, kStartCounts[i] - counts[0][i]));
    material += (counts[0][i] - counts[1][i]) * kPieceValues[i];
  }

  // The analysis and its hint were about the position before.
  analysis = Analysis{};
  hintRequested = false;

  // Two players with a turning board: the side to move sits at the bottom.
  if (kind == GameKind::TwoPlayers && autoFlip && flipped == status.white_to_move) {
    flipped = !status.white_to_move;
    nextRefresh = HalDisplay::HALF_REFRESH;  // the whole board changes
  }
}

// `replaying` keeps the moves taken back for Redo; any other move ends them.
void ChessActivity::play(const uint16_t move, const bool replaying) {
  San san{};
  cc_game_san(&game, move, san.data(), san.size());
  if (!cc_game_play(&game, move)) return;
  if (!replaying) redoMoves.clear();
  sanMoves.push_back(san);
  refreshPosition();
  selected = -1;
  promotionTo = -1;
  showNoMoves = false;
  cursor = defaultCursor();
  // An engine reply can land while a card is open: keep the card, unless the
  // game just ended and the result should show.
  if (isOver()) {
    menuChoice = 0;  // the menu just got shorter
    if (mode != Mode::NewGame) mode = Mode::Menu;
  } else if (mode != Mode::Menu && mode != Mode::NewGame) {
    mode = Mode::PickPiece;
  }
  if (--movesUntilHalfRefresh <= 0) {
    nextRefresh = HalDisplay::HALF_REFRESH;
    movesUntilHalfRefresh = SETTINGS.getRefreshFrequency();
  }
}

bool ChessActivity::undo() {
  const uint16_t move = lastMove;
  if (!cc_game_undo(&game)) return false;
  redoMoves.push_back(move);
  if (!sanMoves.empty()) sanMoves.pop_back();
  refreshPosition();
  return true;
}

// Against the engine, takes back its reply too, so it is the player's turn.
// Also lets an engine that could not start try again.
void ChessActivity::takeBack() {
  engineFailedAt = -1;
  if (!undo()) return;
  if (engineToMove() && status.plies > 0) undo();
  mode = Mode::PickPiece;
  selected = -1;
  cursor = defaultCursor();
}

// Replays what was taken back; against the engine its reply too, so it is
// the player's turn again.
void ChessActivity::redo() {
  if (redoMoves.empty()) return;
  play(redoMoves.back(), true);
  redoMoves.pop_back();
  if (engineToMove() && !redoMoves.empty()) {
    play(redoMoves.back(), true);
    redoMoves.pop_back();
  }
  engineFailedAt = -1;
}

bool ChessActivity::ownPiece(const int square) const {
  const uint8_t piece = squares[square];
  return piece != 0 && ((piece & CC_BLACK) == 0) == status.white_to_move;
}

// The piece the side to move moved last, else its king's pawn, else its king.
int ChessActivity::defaultCursor() const {
  if (status.plies >= 2) {
    const int square = cc_move_to(cc_game_move(&game, status.plies - 2));
    if (ownPiece(square)) return square;
  }
  const int kingPawn = status.white_to_move ? 12 : 52;  // e2 / e7
  if (ownPiece(kingPawn)) return kingPawn;
  for (int square = 0; square < 64; square++) {
    if (ownPiece(square) && (squares[square] & 7) == CC_KING) return square;
  }
  return 0;
}

uint64_t ChessActivity::targetsOf(const int from) const {
  uint64_t targets = 0;
  for (int i = 0; i < legalCount; i++) {
    if (cc_move_from(legalMoves[i]) == from) targets |= uint64_t{1} << cc_move_to(legalMoves[i]);
  }
  return targets;
}

uint16_t ChessActivity::findMove(const int from, const int to, const int promotion) const {
  for (int i = 0; i < legalCount; i++) {
    const uint16_t move = legalMoves[i];
    if (cc_move_from(move) == from && cc_move_to(move) == to && cc_move_promotion(move) == promotion) return move;
  }
  return 0;
}

// --- Engine ----------------------------------------------------------------

bool ChessActivity::ensureEngine() {
  if (engine != nullptr) return true;
  auto* created = new (std::nothrow) ChessEngine();
  if (created == nullptr) return false;
  if (xTaskCreate(engineTask, "ChessEngine", kEngineStackBytes, created, 1, &created->task) != pdPASS) {
    delete created;
    return false;
  }
  engine = created;
  return true;
}

// Starts the opponent's search, or with `analyse` a full-strength analysis
// of the player's position for training.
void ChessActivity::startSearch(const bool analyse) {
  if (!ensureEngine()) {
    LOG_ERR("CHESS", "No memory for the engine");
    RenderLock lock;
    engineFailedAt = status.plies;
    return;
  }
  const Level& settings = kLevels[std::clamp(level, 1, kLevelCount) - 1];
  engine->game = game;
  engine->analysis = analyse;
  engine->plies = status.plies;
  engine->lastMove = lastMove;
  engine->limits = analyse ? cc_search_limits{kAnalysisDepth, 0, esp_random(), engineShouldStop, engine}
                           : cc_search_limits{settings.depth, settings.noise, esp_random(), engineShouldStop, engine};
  engine->startedMs = millis();
  engine->lastYieldMs = engine->startedMs;
  engine->budgetMs = analyse ? kAnalysisMs : settings.thinkMs;
  engine->cancel.store(false, std::memory_order_relaxed);
  engine->busy.store(true, std::memory_order_release);
  xTaskNotifyGive(engine->task);
  RenderLock lock;
  searching = true;
  thinking = !analyse;
}

// From loop(): the render task reads `thinking`, so clear it under the lock.
void ChessActivity::cancelSearch() {
  if (!searching) return;
  waitForIdle(*engine);
  RenderLock lock;
  searching = false;
  thinking = false;
}

// For onExit and the destructor, which run while the activity manager holds
// the render lock: taking it again here would deadlock. Deleting the idle task
// from here frees its stack at once.
void ChessActivity::shutDownEngine() {
  if (engine == nullptr) return;
  if (searching) waitForIdle(*engine);
  searching = false;
  thinking = false;
  vTaskDelete(engine->task);
  delete engine;
  engine = nullptr;
}

void ChessActivity::pollEngine() {
  if (!searching || engine->busy.load(std::memory_order_acquire)) return;
  const bool analyse = engine->analysis;
  if (!analyse && millis() - engine->startedMs < kMinThinkMs) return;

  const cc_search_result result = engine->result;
  const bool current = engine->plies == status.plies && engine->lastMove == lastMove;
  char uci[6] = "-";
  if (result.move != 0) cc_move_uci(result.move, uci);
  LOG_INF("CHESS", "Engine %s level %d: %s, depth %d, score %d, %u nodes in %lu ms, %u bytes of stack unused",
          analyse ? "analysis" : "move", level, uci, result.depth, static_cast<int>(result.score),
          static_cast<unsigned>(result.nodes), millis() - engine->startedMs, static_cast<unsigned>(engine->stackLeft));
  {
    RenderLock lock;
    searching = false;
    thinking = false;
    if (!analyse) {
      if (result.move != 0) play(result.move);
    } else if (current && result.move != 0) {
      analysis.valid = true;
      analysis.whiteScore = status.white_to_move ? result.score : -result.score;
      analysis.best = result.move;
      cc_game_san(&game, result.move, analysis.bestSan.data(), analysis.bestSan.size());
      // A hint asked for while the engine was still looking: point at it.
      if (hintRequested && mode == Mode::PickPiece) cursor = cc_move_from(result.move);
    }
  }
  if (!analyse && result.move != 0) save();
  requestUpdate();
}

// On the engine's turn, starts its search (dropping a training analysis of
// the position before). In training, on the player's turn, analyses the
// position for the evaluation and the hint.
void ChessActivity::scheduleEngine() {
  if (leaving || isOver() || engineFailedAt == status.plies) return;
  if (engineToMove()) {
    if (searching && engine->analysis) cancelSearch();
    if (!searching) {
      startSearch(false);
      requestUpdate();
    }
  } else if (kind == GameKind::Training && !searching && !analysis.valid) {
    startSearch(true);
    if (!searching) requestUpdate();  // show that it failed
  }
}

// --- Input -----------------------------------------------------------------

void ChessActivity::loop() {
  pollEngine();

  // A long Back leaves at once; the game is already saved.
  if (mappedInput.wasLongPressed(MappedInputManager::Button::Back, kLeaveHoldMs)) {
    leave();
    return;
  }
  switch (mode) {
    case Mode::Menu:
      handleMenuInput();
      break;
    case Mode::NewGame:
      handleSetupInput();
      break;
    default:
      handleBoardInput();
      break;
  }
  scheduleEngine();
}

void ChessActivity::leave() {
  leaving = true;
  onGoHome(HomeMenuItem::CHESS);
}

void ChessActivity::handleBoardInput() {
  using Button = MappedInputManager::Button;
  // On the engine's turn only the menu opens.
  if (boardLocked()) {
    if (mappedInput.wasReleased(Button::Back)) handleBack();
    return;
  }
  if (mappedInput.wasPressed(Button::ScreenLeft)) {
    handleArrow(-1, 0);
  } else if (mappedInput.wasPressed(Button::ScreenRight)) {
    handleArrow(1, 0);
  } else if (mappedInput.wasPressed(Button::ScreenUp)) {
    handleArrow(0, -1);
  } else if (mappedInput.wasPressed(Button::ScreenDown)) {
    handleArrow(0, 1);
  } else if (mappedInput.wasPressed(Button::Confirm)) {
    handleConfirm();
  } else if (mappedInput.wasReleased(Button::Back)) {
    handleBack();
  }
}

void ChessActivity::handleMenuInput() {
  using Button = MappedInputManager::Button;
  const bool previous = mappedInput.wasPressed(Button::Up) || mappedInput.wasPressed(Button::Left);
  const bool next = mappedInput.wasPressed(Button::Down) || mappedInput.wasPressed(Button::Right);
  if (previous || next) {
    {
      RenderLock lock;
      menuChoice = (menuChoice + (next ? 1 : menuCount() - 1)) % menuCount();
    }
    requestUpdate();
  } else if (mappedInput.wasPressed(Button::Confirm)) {
    runMenuItem(menuItem(menuChoice));
  } else if (mappedInput.wasReleased(Button::Back)) {
    {
      RenderLock lock;
      mode = Mode::PickPiece;
    }
    requestUpdate();
  }
}

// Up and Down move between the rows, Left and Right change the value,
// Confirm starts the game from any row.
void ChessActivity::handleSetupInput() {
  using Button = MappedInputManager::Button;
  const int rows = setupRowCount();
  if (mappedInput.wasPressed(Button::Up) || mappedInput.wasPressed(Button::Down)) {
    {
      RenderLock lock;
      setupRow = (setupRow + (mappedInput.wasPressed(Button::Down) ? 1 : rows - 1)) % rows;
    }
    requestUpdate();
  } else if (mappedInput.wasPressed(Button::Left) || mappedInput.wasPressed(Button::Right)) {
    const int step = mappedInput.wasPressed(Button::Right) ? 1 : -1;
    {
      RenderLock lock;
      if (setupRow == 0) {
        setupKind = static_cast<GameKind>((static_cast<int>(setupKind) + 3 + step) % 3);
      } else if (setupKind == GameKind::TwoPlayers) {
        if (setupRow == 1) setupAutoFlip = !setupAutoFlip;
      } else if (setupRow == 1) {
        setupSide = static_cast<SideChoice>((static_cast<int>(setupSide) + 3 + step) % 3);
      } else if (setupRow == 2) {
        setupLevel = std::clamp(setupLevel + step, 1, kLevelCount);
      }
      setupRow = std::min(setupRow, setupRowCount() - 1);
    }
    requestUpdate();
  } else if (mappedInput.wasPressed(Button::Confirm)) {
    startNewGame();
  } else if (mappedInput.wasReleased(Button::Back)) {
    {
      RenderLock lock;
      mode = setupReturn;
      menuChoice = 0;
    }
    requestUpdate();
  }
}

void ChessActivity::handleArrow(const int dx, const int dy) {
  {
    RenderLock lock;
    showNoMoves = false;
    switch (mode) {
      case Mode::PickPiece: {
        uint64_t own = 0;
        for (int square = 0; square < 64; square++) {
          if (ownPiece(square)) own |= uint64_t{1} << square;
        }
        cursor = chesscursor::next(cursor, dx, dy, own, flipped);
        break;
      }
      case Mode::PickTarget:
        cursor = chesscursor::next(cursor, dx, dy, targetsOf(selected), flipped);
        break;
      case Mode::Promotion: {
        // Arrows along the picker move through it; the other axis too, so
        // every arrow does something.
        const int step = promotionStep();
        const int delta = (dy != 0) ? dy * step : dx;
        promotionChoice = (promotionChoice + delta + 4) % 4;
        break;
      }
      case Mode::Menu:
      case Mode::NewGame:
        break;
    }
  }
  requestUpdate();
}

void ChessActivity::handleConfirm() {
  if (isOver()) {
    RenderLock lock;
    mode = Mode::Menu;
    menuChoice = 0;
  } else if (mode == Mode::PickPiece) {
    RenderLock lock;
    const uint64_t targets = targetsOf(cursor);
    if (!ownPiece(cursor) || targets == 0) {
      showNoMoves = true;
    } else {
      selected = cursor;
      mode = Mode::PickTarget;
      // Start on the hinted target, else on the nearest one toward the opponent.
      const bool upward = status.white_to_move != flipped;
      cursor = hintShown() && cc_move_from(analysis.best) == selected
                   ? cc_move_to(analysis.best)
                   : chesscursor::next(selected, 0, upward ? -1 : 1, targets, flipped);
    }
  } else if (mode == Mode::PickTarget) {
    if (findMove(selected, cursor, CC_QUEEN) != 0) {
      RenderLock lock;
      mode = Mode::Promotion;
      promotionTo = cursor;
      promotionChoice = 0;
    } else if (const uint16_t move = findMove(selected, cursor, 0)) {
      {
        RenderLock lock;
        play(move);
      }
      save();
    }
  } else if (mode == Mode::Promotion) {
    if (const uint16_t move = findMove(selected, promotionTo, kPromotionKinds[promotionChoice])) {
      {
        RenderLock lock;
        play(move);
      }
      save();
    }
  }
  requestUpdate();
}

void ChessActivity::handleBack() {
  {
    RenderLock lock;
    showNoMoves = false;
    switch (mode) {
      case Mode::PickPiece:
        mode = Mode::Menu;
        menuChoice = 0;
        break;
      case Mode::PickTarget:
        mode = Mode::PickPiece;
        cursor = selected;
        selected = -1;
        break;
      case Mode::Promotion:
        mode = Mode::PickTarget;
        cursor = promotionTo;
        promotionTo = -1;
        break;
      case Mode::Menu:
      case Mode::NewGame:
        break;
    }
  }
  requestUpdate();
}

ChessActivity::MenuItem ChessActivity::menuItem(const int index) const {
  static constexpr MenuItem kPlaying[] = {MenuItem::Continue, MenuItem::Flip, MenuItem::TakeBack, MenuItem::NewGame,
                                          MenuItem::Exit};
  // Training puts its tools first; Back already continues.
  static constexpr MenuItem kTraining[] = {MenuItem::Hint, MenuItem::TakeBack, MenuItem::Redo,
                                           MenuItem::Flip, MenuItem::NewGame,  MenuItem::Exit};
  static constexpr MenuItem kOver[] = {MenuItem::NewGame, MenuItem::TakeBack, MenuItem::Flip, MenuItem::Exit};
  const int i = std::clamp(index, 0, menuCount() - 1);
  if (isOver()) return kOver[i];
  return kind == GameKind::Training ? kTraining[i] : kPlaying[i];
}

void ChessActivity::runMenuItem(const MenuItem item) {
  switch (item) {
    case MenuItem::Exit:
      leave();
      return;
    case MenuItem::NewGame: {
      RenderLock lock;
      openSetup(Mode::Menu);
      break;
    }
    case MenuItem::Hint: {
      // Shown as soon as the analysis has it; the cursor goes to the piece.
      RenderLock lock;
      hintRequested = true;
      mode = Mode::PickPiece;
      if (analysis.valid && analysis.best != 0) cursor = cc_move_from(analysis.best);
      break;
    }
    case MenuItem::TakeBack:
    case MenuItem::Redo: {
      // The search works on a position that is about to go away.
      cancelSearch();
      {
        RenderLock lock;
        if (item == MenuItem::TakeBack) {
          takeBack();
        } else {
          redo();
          if (!isOver()) mode = Mode::PickPiece;
        }
        nextRefresh = HalDisplay::HALF_REFRESH;
      }
      save();
      break;
    }
    case MenuItem::Flip: {
      {
        RenderLock lock;
        flipped = !flipped;
        mode = isOver() ? Mode::Menu : Mode::PickPiece;
        nextRefresh = HalDisplay::HALF_REFRESH;
      }
      save();
      break;
    }
    case MenuItem::Continue: {
      RenderLock lock;
      mode = Mode::PickPiece;
      break;
    }
  }
  requestUpdate();
}

// The card starts from the current game's choices.
void ChessActivity::openSetup(const Mode returnTo) {
  setupKind = kind;
  setupSide = playerWhite ? SideChoice::White : SideChoice::Black;
  setupLevel = level;
  setupAutoFlip = autoFlip;
  setupRow = 0;
  setupReturn = returnTo;
  mode = Mode::NewGame;
}

void ChessActivity::startNewGame() {
  cancelSearch();
  {
    RenderLock lock;
    kind = setupKind;
    level = setupLevel;
    autoFlip = setupAutoFlip;
    playerWhite = setupSide == SideChoice::Random ? (esp_random() & 1) != 0 : setupSide == SideChoice::White;
    // Against the engine the player sits at the bottom; two players start
    // with White there.
    flipped = againstEngine() && !playerWhite;
    newGame();
    nextRefresh = HalDisplay::HALF_REFRESH;
  }
  save();
  requestUpdate();
}

// +1 when the promotion picker grows down the screen, -1 when it grows up.
int ChessActivity::promotionStep() const { return screenRow(promotionTo) < 4 ? 1 : -1; }

// --- Drawing ---------------------------------------------------------------

int ChessActivity::screenCol(const int square) const { return chesscursor::screenCol(square, flipped); }
int ChessActivity::screenRow(const int square) const { return chesscursor::screenRow(square, flipped); }

void ChessActivity::render(RenderLock&&) {
  renderer.clearScreen();

  const auto& metrics = UITheme::getInstance().getMetrics();
  const int pageWidth = renderer.getScreenWidth();
  const int pageHeight = renderer.getScreenHeight();

  // The short header band home uses, so the board can take 64 px squares.
  const int headerBottom = metrics.homeTopPadding;
  char moveNumber[32];
  snprintf(moveNumber, sizeof(moveNumber), tr(STR_CHESS_MOVE_NUMBER), status.fullmove_number);
  const char* title = kind == GameKind::Training     ? tr(STR_CHESS_TRAINING)
                      : kind == GameKind::TwoPlayers ? tr(STR_CHESS_TWO_PLAYERS)
                                                     : tr(STR_CHESS_GAME);
  GUI.drawHeader(renderer, Rect{0, metrics.topPadding, pageWidth, headerBottom - metrics.topPadding}, title,
                 moveNumber);

  const int topBarY = headerBottom + kGap;
  const int boardTop = topBarY + kBarHeight + kGap;
  const int belowBoard = kGap + kBarHeight + kGap + kMinStripHeight + metrics.buttonHintsHeight;
  const int squareSize = std::min(pageWidth - 2 * kBoardMargin, pageHeight - boardTop - belowBoard) / 8;
  const int boardX = (pageWidth - 8 * squareSize) / 2;
  const int bottomBarY = boardTop + 8 * squareSize + kGap;
  int stripY = bottomBarY + kBarHeight + kGap;
  int stripHeight = pageHeight - metrics.buttonHintsHeight - stripY;

  // The side at the bottom of the board plays from the bottom bar.
  drawBar(topBarY, flipped);
  drawBoard(boardX, boardTop, squareSize);
  drawBar(bottomBarY, !flipped);
  if (kind == GameKind::Training) {
    // The evaluation takes the strip's first line.
    drawTrainingLine(stripY);
    stripY += kStripLineHeight;
    stripHeight -= kStripLineHeight;
  }
  drawMoveStrip(stripY, stripHeight);

  if (mode == Mode::Promotion) drawPromotionPicker(boardX, boardTop, squareSize);
  if (mode == Mode::Menu) drawMenu(boardX, boardTop, squareSize);
  if (mode == Mode::NewGame) drawSetup(boardX, boardTop, squareSize);

  MappedInputManager::Labels labels{};
  switch (mode) {
    case Mode::PickPiece:
      labels = boardLocked() ? mappedInput.mapLabels(tr(STR_CHESS_MENU), "", "", "")
                             : mappedInput.mapDirectionalLabels(tr(STR_CHESS_MENU), tr(STR_SELECT), tr(STR_DIR_LEFT),
                                                                tr(STR_DIR_RIGHT), tr(STR_DIR_UP), tr(STR_DIR_DOWN));
      break;
    case Mode::PickTarget:
      labels = mappedInput.mapDirectionalLabels(tr(STR_CANCEL), tr(STR_CHESS_PLAY_MOVE), tr(STR_DIR_LEFT),
                                                tr(STR_DIR_RIGHT), tr(STR_DIR_UP), tr(STR_DIR_DOWN));
      break;
    case Mode::Promotion:
      labels = mappedInput.mapDirectionalLabels(tr(STR_CANCEL), tr(STR_SELECT), tr(STR_DIR_LEFT), tr(STR_DIR_RIGHT),
                                                tr(STR_DIR_UP), tr(STR_DIR_DOWN));
      break;
    case Mode::Menu:
      labels = mappedInput.mapLabels(tr(STR_BACK), tr(STR_SELECT), tr(STR_DIR_UP), tr(STR_DIR_DOWN));
      break;
    case Mode::NewGame:
      labels = mappedInput.mapLabels(tr(STR_CANCEL), tr(STR_CHESS_START), "<", ">");
      break;
  }
  GUI.drawButtonHints(renderer, labels.btn1, labels.btn2, labels.btn3, labels.btn4);

  renderer.displayBuffer(nextRefresh);
  nextRefresh = HalDisplay::FAST_REFRESH;
}

void ChessActivity::drawBoard(const int x, const int y, const int squareSize) const {
  const int boardSize = 8 * squareSize;
  renderer.drawRect(x - kFrameWidth, y - kFrameWidth, boardSize + 2 * kFrameWidth, boardSize + 2 * kFrameWidth,
                    kFrameWidth, true);

  // Targets of the picked-up piece: empty squares get a dot, captures get corners.
  uint64_t quiet = 0;
  uint64_t captures = 0;
  if (selected >= 0) {
    for (int i = 0; i < legalCount; i++) {
      const uint16_t move = legalMoves[i];
      if (cc_move_from(move) != selected) continue;
      const int to = cc_move_to(move);
      const bool capture = squares[to] != 0 || cc_move_kind(move) == CC_MOVE_EN_PASSANT;
      (capture ? captures : quiet) |= uint64_t{1} << to;
    }
  }

  for (int square = 0; square < 64; square++) {
    const int squareX = x + screenCol(square) * squareSize;
    const int squareY = y + screenRow(square) * squareSize;
    drawSquare(squareX, squareY, squareSize, square, quiet, captures);
  }

  if (hintShown()) drawHintArrow(x, y, squareSize);

  // The cursor goes on top of everything but the coordinates; it is hidden
  // while a dialog covers the board.
  if ((mode == Mode::PickPiece && !boardLocked()) || mode == Mode::PickTarget) {
    drawCursor(renderer, x + screenCol(cursor) * squareSize, y + screenRow(cursor) * squareSize, squareSize);
  }

  if (squareSize < kMinCoordinateSquare) return;
  const int labelHeight = renderer.getLineHeight(SMALL_FONT_ID);
  for (int i = 0; i < 8; i++) {
    // Screen column/row i shows file/rank i, or 7 - i when flipped.
    const int index = flipped ? 7 - i : i;
    const char file[2] = {static_cast<char>('a' + index), '\0'};
    const char rank[2] = {static_cast<char>('8' - index), '\0'};
    drawCoordinate(renderer, x + (i + 1) * squareSize - renderer.getTextWidth(SMALL_FONT_ID, file) - 4,
                   y + boardSize - labelHeight, file);
    drawCoordinate(renderer, x + 4, y + i * squareSize + 1, rank);
  }
}

// One square, back to front as in design/README.md section 4.
void ChessActivity::drawSquare(const int x, const int y, const int squareSize, const int square, const uint64_t quiet,
                               const uint64_t captures) const {
  const bool dark = (square % 8 + square / 8) % 2 == 0;  // a1 is dark
  if (dark) drawHatch(renderer, x, y, squareSize);

  if (lastMove != 0 && (square == cc_move_from(lastMove) || square == cc_move_to(lastMove))) {
    // A 50% ring, on white so it reads on hatch too.
    const int ring = std::max(2, squareSize / 10);
    const int inner = squareSize - ring;
    const int sides[4][4] = {{x, y, squareSize, ring},
                             {x, y + inner, squareSize, ring},
                             {x, y, ring, squareSize},
                             {x + inner, y, ring, squareSize}};
    for (const auto& side : sides) {
      renderer.fillRect(side[0], side[1], side[2], side[3], false);
      renderer.fillRectDither(side[0], side[1], side[2], side[3], Color::DarkGray);
    }
  }

  const uint8_t piece = squares[square];
  if (status.in_check && ownPiece(square) && (piece & 7) == CC_KING) drawCheckGlow(renderer, x, y, squareSize);
  if (hasBit(captures, square)) drawCaptureMarks(renderer, x, y, squareSize);
  if (piece != 0) drawPiece(x, y, squareSize, piece);
  if (hasBit(quiet, square)) {
    const int radius = squareSize / 8;
    fillDisc(renderer, x + squareSize / 2, y + squareSize / 2, radius + 2, false);
    fillDisc(renderer, x + squareSize / 2, y + squareSize / 2, radius, true);
  }
  if (square == selected) drawSelectionFrame(renderer, x, y, squareSize);
}

void ChessActivity::drawPiece(const int x, const int y, const int squareSize, const uint8_t piece) const {
  const int kind = piece & 7;
  if (kind < CC_PAWN || kind > CC_KING) return;
  const auto& set = pieceSetFor(squareSize);
  const auto& glyph = set.glyphs[kind - CC_PAWN];
  const int inset = (squareSize - set.squareSize) / 2;
  drawGlyph(renderer, glyph, glyphBitmap::Frame{x + inset + glyph.left, y + inset + glyph.top, 1, 0, 0, 1},
            piece & CC_BLACK, false);
}

// Token, name, the pieces this side has taken, its material lead and a pill:
// whose turn it is, or the result.
void ChessActivity::drawBar(const int y, const bool white) const {
  const bool toMove = status.white_to_move == white;
  const bool engineSide = againstEngine() && white != playerWhite;

  // The pill first, right aligned; the rest of the bar stops short of it.
  const char* pill = nullptr;
  if (status.outcome == CC_CHECKMATE) {
    if (!toMove) pill = tr(STR_CHESS_WINNER);
  } else if (isOver()) {
    pill = tr(STR_CHESS_DRAW);
  } else if (toMove && engineSide) {
    pill = thinking ? tr(STR_CHESS_THINKING) : tr(STR_CHESS_TO_MOVE);
  } else if (toMove) {
    pill = status.in_check ? tr(STR_CHESS_CHECK) : againstEngine() ? tr(STR_CHESS_YOUR_MOVE) : tr(STR_CHESS_TO_MOVE);
  }
  int right = renderer.getScreenWidth() - kPillInset;
  if (pill != nullptr) {
    const int pillWidth = renderer.getTextWidth(UI_10_FONT_ID, pill, EpdFontFamily::BOLD) + 2 * kPillInset;
    const int pillX = right - pillWidth;
    const int pillY = y + (kBarHeight - kPillHeight) / 2;
    renderer.fillRoundedRect(pillX, pillY, pillWidth, kPillHeight, kPillHeight / 2, Color::Black);
    renderer.drawText(UI_10_FONT_ID, pillX + kPillInset,
                      pillY + (kPillHeight - renderer.getLineHeight(UI_10_FONT_ID)) / 2, pill, false,
                      EpdFontFamily::BOLD);
    right = pillX - 8;
  }

  const int tokenY = y + (kBarHeight - kTokenSize) / 2;
  renderer.fillRoundedRect(14, tokenY, kTokenSize, kTokenSize, 4, Color::Black);
  if (white) renderer.fillRoundedRect(16, tokenY + 2, kTokenSize - 4, kTokenSize - 4, 2, Color::White);

  // Against the engine the bars say who is who; between two players, which colour.
  const char* name = kind == GameKind::TwoPlayers ? (white ? tr(STR_CHESS_WHITE) : tr(STR_CHESS_BLACK))
                     : engineSide                 ? tr(STR_CHESS_COMPUTER)
                                                  : tr(STR_CHESS_YOU);
  const auto style = toMove && !isOver() ? EpdFontFamily::BOLD : EpdFontFamily::REGULAR;
  const int nameY = y + (kBarHeight - renderer.getLineHeight(UI_12_FONT_ID)) / 2;
  renderer.drawText(UI_12_FONT_ID, kNameX, nameY, name, true, style);
  int x = kNameX + renderer.getTextWidth(UI_12_FONT_ID, name, style) + 12;
  const int smallY = y + (kBarHeight - renderer.getLineHeight(UI_10_FONT_ID)) / 2;
  if (engineSide) {
    char levelText[32];
    snprintf(levelText, sizeof(levelText), tr(STR_CHESS_LEVEL_N), level);
    const int width = renderer.getTextWidth(UI_10_FONT_ID, levelText);
    if (x + width <= right) renderer.drawText(UI_10_FONT_ID, x, smallY, levelText);
    x += width + 12;
  }

  // Taken pieces, as many as fit, then the material lead.
  const int side = white ? 0 : 1;
  const int figurineSize = chesspieces::kSet24.squareSize;
  const int figurineY = y + (kBarHeight - figurineSize) / 2;
  for (int kind = 0; kind < 5 && x + figurineSize <= right; kind++) {
    for (int i = 0; i < captured[side][kind] && x + figurineSize <= right; i++) {
      drawFigurine(renderer, x, figurineY, kind + CC_PAWN, white);
      x += i + 1 < captured[side][kind] ? kFigurineStep : figurineSize;
    }
  }
  const int lead = white ? material : -material;
  if (lead > 0) {
    char text[8];
    snprintf(text, sizeof(text), "+%d", lead);
    if (x + 2 + renderer.getTextWidth(UI_10_FONT_ID, text) <= right)
      renderer.drawText(UI_10_FONT_ID, x + 2, smallY, text);
  }
}

// SAN with figurines in place of piece letters. Returns the width; draws only
// when `draw` is set, so the same code measures.
int ChessActivity::drawSan(const int x, const int y, const char* san, const bool inverted, const bool draw) const {
  const int figurineY = y + (renderer.getLineHeight(UI_12_FONT_ID) - chesspieces::kSet24.squareSize) / 2;
  int width = 0;
  char run[sizeof(San)];
  size_t runLength = 0;
  const auto flush = [&] {
    if (runLength == 0) return;
    run[runLength] = '\0';
    if (draw) renderer.drawText(UI_12_FONT_ID, x + width, y, run, !inverted);
    width += renderer.getTextWidth(UI_12_FONT_ID, run);
    runLength = 0;
  };
  for (const char* c = san; *c != '\0' && runLength + 1 < sizeof(run); c++) {
    const int kind = pieceKindForLetter(*c);
    if (kind == 0) {
      run[runLength++] = *c;
      continue;
    }
    flush();
    if (draw) drawFigurine(renderer, x + width, figurineY, kind, false, inverted);
    width += chesspieces::kSet24.squareSize - 2;
  }
  flush();
  return width;
}

// The newest moves that fit, oldest first, newest in a black pill. A line
// breaks only before a move number.
void ChessActivity::drawMoveStrip(const int y, const int height) const {
  const int left = kBoardMargin + 4;
  const int right = renderer.getScreenWidth() - kBoardMargin - 4;
  const int lineHeight = renderer.getLineHeight(UI_12_FONT_ID);
  const int lines = std::max(1, height / kStripLineHeight);
  const int textOffset = (kStripLineHeight - lineHeight) / 2;

  const bool engineFailed = engineFailedAt == status.plies && engineToMove();
  if (showNoMoves || engineFailed) {
    renderer.drawCenteredText(UI_12_FONT_ID, y + textOffset,
                              showNoMoves ? tr(STR_CHESS_NO_MOVES) : tr(STR_CHESS_ENGINE_FAILED));
    return;
  }
  const int plies = static_cast<int>(sanMoves.size());
  if (plies == 0) return;

  // Count plies from White's first move of the game, so a game that starts
  // with Black to move still numbers its moves as the FEN says.
  const int currentPly = (std::max<int>(status.fullmove_number, 1) - 1) * 2 + (status.white_to_move ? 0 : 1);
  const int firstPly = std::max(0, currentPly - plies);
  constexpr int kNumberGap = 4;
  constexpr int kMoveGap = 10;
  constexpr int kUnitGap = 14;

  struct Unit {
    int ply;    // first ply of the unit
    int count;  // 1 or 2 plies
    int width;
  };
  constexpr int kMaxUnits = 24;
  Unit units[kMaxUnits];
  int unitCount = 0;

  const auto label = [&](const int ply, char* out, const size_t size) {
    const int absolute = firstPly + ply;
    snprintf(out, size, absolute % 2 == 0 ? "%d." : "%d...", absolute / 2 + 1);
  };

  // Walk back from the newest move, one move number at a time.
  for (int end = plies; end > 0 && unitCount < kMaxUnits;) {
    int start = end - 1;
    if ((firstPly + start) % 2 == 1 && start > 0) start--;
    char number[12];
    label(start, number, sizeof(number));
    int width = renderer.getTextWidth(UI_12_FONT_ID, number) + kNumberGap;
    for (int ply = start; ply < end; ply++) {
      width += drawSan(0, 0, sanMoves[ply].data(), false, false) + (ply + 1 < end ? kMoveGap : 0);
    }
    units[unitCount++] = Unit{start, end - start, width};
    end = start;
  }

  // Keep as many of the newest units as fit in the strip's lines.
  const auto linesFor = [&](const int count) {
    int used = 1;
    int x = left;
    for (int i = count - 1; i >= 0; i--) {
      if (x > left && x + units[i].width > right) {
        used++;
        x = left;
      }
      x += units[i].width + kUnitGap;
    }
    return used;
  };
  int shown = unitCount;
  while (shown > 1 && linesFor(shown) > lines) shown--;

  int x = left;
  int line = 0;
  for (int i = shown - 1; i >= 0; i--) {
    const Unit& unit = units[i];
    if (x > left && x + unit.width > right) {
      line++;
      x = left;
    }
    const int textY = y + line * kStripLineHeight + textOffset;
    char number[12];
    label(unit.ply, number, sizeof(number));
    renderer.drawText(UI_12_FONT_ID, x, textY, number);
    int moveX = x + renderer.getTextWidth(UI_12_FONT_ID, number) + kNumberGap;
    for (int ply = unit.ply; ply < unit.ply + unit.count; ply++) {
      const char* san = sanMoves[ply].data();
      const bool newest = ply == plies - 1;
      const int width = drawSan(0, 0, san, false, false);
      if (newest) {
        renderer.fillRoundedRect(moveX - 4, y + line * kStripLineHeight + 2, width + 8, kStripLineHeight - 4, 6,
                                 Color::Black);
      }
      drawSan(moveX, textY, san, newest, true);
      moveX += width + kMoveGap;
    }
    x += unit.width + kUnitGap;
  }
}

// A column of four choices that grows from the promotion square toward the
// centre of the board.
void ChessActivity::drawPromotionPicker(const int boardX, const int boardY, const int squareSize) const {
  const int step = promotionStep();
  const int col = screenCol(promotionTo);
  const int firstRow = screenRow(promotionTo);
  const int topRow = step > 0 ? firstRow : firstRow - 3;
  const int x = boardX + col * squareSize;
  const int top = boardY + topRow * squareSize;

  renderer.fillRect(x + kCardShadow, top + kCardShadow, squareSize, 4 * squareSize, true);
  renderer.fillRect(x, top, squareSize, 4 * squareSize, false);
  renderer.drawRect(x, top, squareSize, 4 * squareSize, 3, true);
  const bool black = !status.white_to_move;
  for (int i = 0; i < 4; i++) {
    const int squareY = boardY + (firstRow + i * step) * squareSize;
    drawPiece(x, squareY, squareSize, kPromotionKinds[i] | (black ? CC_BLACK : 0));
    if (i == promotionChoice) drawSelectionFrame(renderer, x, squareY, squareSize);
  }
}

// The game menu over the board. When the game is over it starts with the
// result: both kings, the loser's lying down.
void ChessActivity::drawMenu(const int boardX, const int boardY, const int squareSize) const {
  const auto& metrics = UITheme::getInstance().getMetrics();
  const int count = menuCount();
  const int rowStep = metrics.menuRowHeight + metrics.menuSpacing;
  const int boardSize = 8 * squareSize;
  const int width = std::min(boardSize - 2 * kCardShadow, 440);
  const int headHeight = isOver() ? 160 : 56;
  const int height = headHeight + count * rowStep;
  const int x = boardX + (boardSize - width) / 2;
  const int y = std::max(boardY - kBarHeight, boardY + (boardSize - height) / 2);
  drawCard(renderer, x, y, width, height);

  if (!isOver()) {
    const char* title = tr(STR_CHESS_MENU);
    const int titleWidth = renderer.getTextWidth(UI_12_FONT_ID, title, EpdFontFamily::BOLD);
    renderer.drawText(UI_12_FONT_ID, x + (width - titleWidth) / 2,
                      y + (headHeight - renderer.getLineHeight(UI_12_FONT_ID)) / 2, title, true, EpdFontFamily::BOLD);
  } else {
    const auto& set = chesspieces::kSet64;
    const bool mate = status.outcome == CC_CHECKMATE;
    const bool whiteWon = mate && !status.white_to_move;
    const int kingsTop = y + 16;
    const int kingsGap = 24;
    const int leftX = x + (width - 2 * set.squareSize - kingsGap) / 2;
    // White's king on the left, Black's on the right; a mated king lies down.
    const auto drawKing = [&](const int cellX, const bool black) {
      const auto& king = set.glyphs[CC_KING - CC_PAWN];
      if (mate && black == whiteWon) {
        // Toppled to the right: glyph x runs down the screen, glyph y runs left.
        const int baseX = cellX + set.squareSize - 1 - king.top;
        const int baseY = kingsTop + (set.squareSize - king.width) / 2;
        drawGlyph(renderer, king, glyphBitmap::Frame{baseX, baseY, 0, 1, -1, 0}, black, false);
      } else {
        drawGlyph(renderer, king, glyphBitmap::Frame{cellX + king.left, kingsTop + king.top, 1, 0, 0, 1}, black, false);
      }
    };
    drawKing(leftX, false);
    drawKing(leftX + set.squareSize + kingsGap, true);

    const char* result = !mate ? tr(STR_CHESS_DRAW)
                         : againstEngine()
                             ? (whiteWon == playerWhite ? tr(STR_CHESS_YOU_WON) : tr(STR_CHESS_COMPUTER_WON))
                         : whiteWon ? tr(STR_CHESS_WHITE_WINS)
                                    : tr(STR_CHESS_BLACK_WINS);
    const int resultY = kingsTop + set.squareSize + 10;
    const int resultWidth = renderer.getTextWidth(NOTOSANS_18_FONT_ID, result, EpdFontFamily::BOLD);
    renderer.drawText(NOTOSANS_18_FONT_ID, x + (width - resultWidth) / 2, resultY, result, true, EpdFontFamily::BOLD);

    char reason[64];
    // Full moves played: Black's reply completes one.
    const int moves = status.fullmove_number - (status.white_to_move ? 1 : 0);
    snprintf(reason, sizeof(reason), tr(STR_CHESS_REASON_MOVES), I18N.get(outcomeReason(status.outcome)), moves);
    const int reasonWidth = renderer.getTextWidth(UI_10_FONT_ID, reason);
    renderer.drawText(UI_10_FONT_ID, x + (width - reasonWidth) / 2,
                      resultY + renderer.getLineHeight(NOTOSANS_18_FONT_ID) + 2, reason);
  }

  GUI.drawButtonMenu(
      renderer, Rect{x, y + headHeight, width, count * rowStep}, count, menuChoice,
      [this](const int index) {
        switch (menuItem(index)) {
          case MenuItem::Continue:
            return std::string(tr(STR_CHESS_CONTINUE));
          case MenuItem::Hint:
            return std::string(tr(STR_CHESS_HINT));
          case MenuItem::Redo:
            return std::string(tr(STR_CHESS_REDO));
          case MenuItem::NewGame:
            return std::string(tr(STR_CHESS_NEW_GAME));
          case MenuItem::TakeBack:
            return std::string(tr(STR_CHESS_TAKE_BACK));
          case MenuItem::Flip:
            return std::string(tr(STR_CHESS_FLIP_BOARD));
          case MenuItem::Exit:
          default:
            return std::string(tr(STR_CHESS_EXIT));
        }
      },
      nullptr);
}

// Training: the evaluation bar and score of the current position, and the
// best move once a hint is asked for.
void ChessActivity::drawTrainingLine(const int y) const {
  constexpr int kEvalBarWidth = 112;
  constexpr int kEvalBarHeight = 14;
  constexpr int kEvalBarRange = 1000;  // centipawns that fill the bar for one side
  if (isOver()) return;

  const int left = kBoardMargin + 4;
  const int textY = y + (kStripLineHeight - renderer.getLineHeight(UI_12_FONT_ID)) / 2;
  const int barY = y + (kStripLineHeight - kEvalBarHeight) / 2;
  renderer.drawRect(left, barY, kEvalBarWidth, kEvalBarHeight, true);
  // A notch over the middle marks a level game.
  renderer.drawLine(left + kEvalBarWidth / 2, barY - 4, left + kEvalBarWidth / 2, barY - 1, true);
  int x = left + kEvalBarWidth + 10;

  if (!analysis.valid) {
    // On the opponent's turn there is nothing to analyse yet; the strip says
    // if its search failed to start.
    if (!engineToMove()) {
      const char* text = engineFailedAt == status.plies ? tr(STR_CHESS_ENGINE_FAILED)
                         : hintRequested                ? tr(STR_CHESS_FINDING_MOVE)
                                                        : tr(STR_CHESS_ANALYSING);
      renderer.drawText(UI_12_FONT_ID, x, textY, text);
    }
    return;
  }
  // White's share from the left, Black's from the right.
  const int inner = kEvalBarWidth - 2;
  const int clamped = std::clamp(analysis.whiteScore, -kEvalBarRange, kEvalBarRange);
  const int whiteWidth = inner * (clamped + kEvalBarRange) / (2 * kEvalBarRange);
  renderer.fillRect(left + 1 + whiteWidth, barY + 1, inner - whiteWidth, kEvalBarHeight - 2, true);

  char score[16];
  formatScore(analysis.whiteScore, score, sizeof(score));
  renderer.drawText(UI_12_FONT_ID, x, textY, score, true, EpdFontFamily::BOLD);
  x += renderer.getTextWidth(UI_12_FONT_ID, score, EpdFontFamily::BOLD) + 18;

  if (hintShown()) {
    const char* label = tr(STR_CHESS_BEST_MOVE);
    renderer.drawText(UI_12_FONT_ID, x, textY, label);
    x += renderer.getTextWidth(UI_12_FONT_ID, label) + 8;
    drawSan(x, textY, analysis.bestSan.data(), false, true);
  }
}

// The hint as an arrow from the piece toward its square: a white outline
// first so it reads on hatch and on pieces, then the ink.
void ChessActivity::drawHintArrow(const int boardX, const int boardY, const int squareSize) const {
  const int from = cc_move_from(analysis.best);
  const int to = cc_move_to(analysis.best);
  const float half = squareSize / 2.0f;
  const float fromX = boardX + screenCol(from) * squareSize + half;
  const float fromY = boardY + screenRow(from) * squareSize + half;
  const float toX = boardX + screenCol(to) * squareSize + half;
  const float toY = boardY + screenRow(to) * squareSize + half;
  const float length = std::hypot(toX - fromX, toY - fromY);
  if (length < 1.0f) return;
  const float alongX = (toX - fromX) / length;
  const float alongY = (toY - fromY) / length;
  const float acrossX = -alongY;
  const float acrossY = alongX;

  const float tipX = toX - alongX * squareSize * 0.12f;
  const float tipY = toY - alongY * squareSize * 0.12f;
  const float tailX = fromX + alongX * squareSize * 0.3f;
  const float tailY = fromY + alongY * squareSize * 0.3f;
  const float headLength = squareSize * 0.4f;
  const float headHalf = squareSize * 0.24f;
  const float baseX = tipX - alongX * headLength;
  const float baseY = tipY - alongY * headLength;
  const auto px = [](const float v) { return static_cast<int>(std::lround(v)); };

  for (const bool ink : {false, true}) {
    const float grow = ink ? 0.0f : 3.0f;
    const float shaftHalf = squareSize * 0.05f + grow;
    const int shaftX[4] = {px(tailX + acrossX * shaftHalf - alongX * grow), px(baseX + acrossX * shaftHalf),
                           px(baseX - acrossX * shaftHalf), px(tailX - acrossX * shaftHalf - alongX * grow)};
    const int shaftY[4] = {px(tailY + acrossY * shaftHalf - alongY * grow), px(baseY + acrossY * shaftHalf),
                           px(baseY - acrossY * shaftHalf), px(tailY - acrossY * shaftHalf - alongY * grow)};
    renderer.fillPolygon(shaftX, shaftY, 4, ink);
    const float wing = headHalf + grow;
    const float backX = baseX - alongX * grow;
    const float backY = baseY - alongY * grow;
    const int headX[3] = {px(tipX + alongX * grow), px(backX + acrossX * wing), px(backX - acrossX * wing)};
    const int headY[3] = {px(tipY + alongY * grow), px(backY + acrossY * wing), px(backY - acrossY * wing)};
    renderer.fillPolygon(headX, headY, 3, ink);
  }
}

// The new-game card: the kind of game, then colour and level against the
// engine or the turning board for two players, then Start.
void ChessActivity::drawSetup(const int boardX, const int boardY, const int squareSize) const {
  const auto& metrics = UITheme::getInstance().getMetrics();
  const int count = setupRowCount();
  const int rowStep = metrics.menuRowHeight + metrics.menuSpacing;
  const int boardSize = 8 * squareSize;
  const int width = std::min(boardSize - 2 * kCardShadow, 440);
  const int headHeight = 56;
  const int height = headHeight + count * rowStep;
  const int x = boardX + (boardSize - width) / 2;
  const int y = boardY + (boardSize - height) / 2;
  drawCard(renderer, x, y, width, height);

  const char* title = tr(STR_CHESS_NEW_GAME);
  const int titleWidth = renderer.getTextWidth(UI_12_FONT_ID, title, EpdFontFamily::BOLD);
  renderer.drawText(UI_12_FONT_ID, x + (width - titleWidth) / 2,
                    y + (headHeight - renderer.getLineHeight(UI_12_FONT_ID)) / 2, title, true, EpdFontFamily::BOLD);

  GUI.drawButtonMenu(
      renderer, Rect{x, y + headHeight, width, count * rowStep}, count, setupRow,
      [this, count](const int index) {
        char row[96];
        if (index == count - 1) {
          snprintf(row, sizeof(row), "%s", tr(STR_CHESS_START));
        } else if (index == 0) {
          const char* value = setupKind == GameKind::Training     ? tr(STR_CHESS_TRAINING)
                              : setupKind == GameKind::TwoPlayers ? tr(STR_CHESS_TWO_PLAYERS)
                                                                  : tr(STR_CHESS_VS_COMPUTER);
          snprintf(row, sizeof(row), "%s:  %s", tr(STR_CHESS_MODE), value);
        } else if (setupKind == GameKind::TwoPlayers) {
          snprintf(row, sizeof(row), "%s:  %s", tr(STR_CHESS_AUTO_FLIP), setupAutoFlip ? tr(STR_YES) : tr(STR_NO));
        } else if (index == 1) {
          const char* side = setupSide == SideChoice::White   ? tr(STR_CHESS_WHITE)
                             : setupSide == SideChoice::Black ? tr(STR_CHESS_BLACK)
                                                              : tr(STR_CHESS_RANDOM);
          snprintf(row, sizeof(row), "%s:  %s", tr(STR_CHESS_YOUR_SIDE), side);
        } else {
          snprintf(row, sizeof(row), "%s:  %d / %d", tr(STR_CHESS_LEVEL), setupLevel, kLevelCount);
        }
        return std::string(row);
      },
      nullptr);
}

#endif  // CROSSCHESS_ENABLED
