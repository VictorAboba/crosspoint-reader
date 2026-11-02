#pragma once

// Keeps an OTA-only device recoverable after a CrossChess build is installed.
//
// Some X3 units can only be reflashed over Wi-Fi or from the SD card, and both
// paths run inside the installed firmware. A build that cannot get through
// startup would leave nothing to install a fix with. The two OTA slots make a
// way out: an update is written to the inactive slot, so the firmware that
// installed this build is still on flash, and the guard boots it when needed.
//
// CROSSCHESS_BOOT_GUARD is set by scripts/build_chess_core.py for the ESP32-C3
// boards only.
namespace crosschess::bootguard {

#ifdef CROSSCHESS_BOOT_GUARD

// Call first thing in setup(), before anything touches the hardware, so a
// start that hangs or crashes anywhere later is already counted. Counts the
// starts of a newly installed build; after kMaxFailedStarts starts that
// neither got healthy (see loop()) nor went to sleep, boots the previous
// firmware. Until the build is healthy, the chip's RTC watchdog resets the
// device when a start shows no sign of life for a minute, so a start that
// hangs counts as failed instead of leaving the device frozen until the
// battery runs out.
void begin();

// Call once the buttons can be read. `sideDownHeld` (Side Down held while
// powering on) switches to the previous firmware right away.
void checkSwitchButton(bool sideDownHeld);

// Call once the SD card is mounted. On the first start of a new build, copies
// the previous firmware and the bootloader region to /crosschess/backup, so the
// previous firmware can still be reinstalled through Recovery mode after a
// later update overwrites its slot.
void backupPreviousFirmware();

// Call from loop(). Marks the running build healthy once it has run a while.
void loop();

// Call right before a deliberate sleep or power-off: a start that ends this
// way did not fail. On the X3 every wake is a cold boot, so without this a
// few short wakes after an update would look like a crash loop. After each
// upstream merge, check that every startDeepSleep call in src/ has this
// call right before it.
void sleeping();

// Call before a deliberate step that can block loop() for longer than the
// watchdog allows: installing a firmware update (a reset would cut it short)
// or getting ready to sleep (plugin delivery can wait on the network). After
// each upstream merge, check that every such path calls this (today
// enterDeepSleep() in main.cpp, SdFirmwareUpdateActivity and
// OtaUpdateActivity).
void stopWatchdog();

#else

inline void begin() {}
inline void checkSwitchButton(bool) {}
inline void backupPreviousFirmware() {}
inline void loop() {}
inline void sleeping() {}
inline void stopWatchdog() {}

#endif

}  // namespace crosschess::bootguard
