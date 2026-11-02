#pragma once

#include <algorithm>
#include <climits>
#include <cstdint>
#include <cstdlib>

// The smart cursor of the chess board. See design/README.md "Smart cursor".
namespace chesscursor {

// Squares count from a1 = 0 to h8 = 63. Screen column 0 is the left edge and
// screen row 0 the top; `flipped` puts Black at the bottom.
inline int screenCol(const int square, const bool flipped) { return flipped ? 7 - square % 8 : square % 8; }
inline int screenRow(const int square, const bool flipped) { return flipped ? square / 8 : 7 - square / 8; }

// The candidate the cursor jumps to from `from` in screen direction (dx, dy),
// one of which is 0 and the other +/-1. Bit n of `candidates` marks square n.
// The lowest along + 2 * across wins; ties go to the smaller "across", then to
// the lower square. When nothing lies that way, the search wraps as if the
// board repeated. Returns `from` when there is no other candidate.
inline int next(const int from, const int dx, const int dy, const uint64_t candidates, const bool flipped) {
  int best = from;
  int bestKey[3] = {INT_MAX, INT_MAX, INT_MAX};
  for (int square = 0; square < 64; square++) {
    if (square == from || ((candidates >> square) & 1) == 0) continue;
    const int offsetX = screenCol(square, flipped) - screenCol(from, flipped);
    const int offsetY = screenRow(square, flipped) - screenRow(from, flipped);
    int along = offsetX * dx + offsetY * dy;
    const int across = std::abs(offsetX * dy - offsetY * dx);
    const bool wrapped = along <= 0;
    if (wrapped) along += 8;
    const int key[3] = {wrapped ? 1 : 0, along + 2 * across, across};
    if (std::lexicographical_compare(key, key + 3, bestKey, bestKey + 3)) {
      std::copy(key, key + 3, bestKey);
      best = square;
    }
  }
  return best;
}

}  // namespace chesscursor
