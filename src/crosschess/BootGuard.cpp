#include "BootGuard.h"

#ifdef CROSSCHESS_BOOT_GUARD

#include <Arduino.h>
#include <HalStorage.h>
#include <Logging.h>
#include <esp_app_desc.h>
#include <esp_flash.h>
#include <esp_image_format.h>
#include <esp_ota_ops.h>
#include <esp_system.h>
#include <hal/wdt_hal.h>
#include <nvs.h>
#include <soc/rtc.h>

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <memory>

#include "network/OtaBootSwitch.h"

namespace crosschess::bootguard {
namespace {

constexpr uint8_t kMaxFailedStarts = 3;
// A build that has run this long got through startup. Later crashes are
// ordinary bugs, and Settings or Recovery mode can still install a fix.
constexpr unsigned long kHealthyAfterMs = 10000;
// A start that shows no sign of life this long has hung. Far above a normal
// setup(); the backup copy feeds the watchdog as it goes.
constexpr uint32_t kWatchdogMs = 60000;
// A copy cut short by a power cycle is tried again once on the next start.
constexpr uint8_t kMaxBackupTries = 2;
static_assert(kMaxBackupTries < kMaxFailedStarts, "backup power cycles alone must not roll back");

constexpr char kNvsNamespace[] = "crosschess";
constexpr char kRecordKey[] = "bootguard";

constexpr char kBackupDir[] = "/crosschess/backup";
// Bootloader and partition table. Stops short of NVS, which holds Wi-Fi passwords.
constexpr uint32_t kBootRegionSize = 0x9000;
constexpr size_t kCopyChunk = 4096;

// Written with every backup, so it always matches the firmware that wrote it.
// Button names follow USER_GUIDE.md.
constexpr char kBackupReadme[] =
    "CrossChess firmware backups\n"
    "\n"
    "When a new CrossChess build starts for the first time, it copies the\n"
    "firmware it replaced to this folder as firmware-<name>-<version>-<id>.bin.\n"
    "Each copy takes about 6 MB; old ones can be deleted.\n"
    "\n"
    "Automatic rollback: if a new build fails to start three times in a row,\n"
    "the device switches back to the previous firmware by itself. A start\n"
    "that freezes restarts after a minute and counts as failed.\n"
    "\n"
    "Both shortcuts below start from off with USB unplugged; with USB power\n"
    "the device starts without checking the buttons. On the X3, Side Up is on\n"
    "the left edge and Side Down on the right.\n"
    "\n"
    "Switch back by hand: hold Side Down, then press and hold Power until the\n"
    "device starts. It keeps starting that firmware until you install the\n"
    "newer build again (or, if that firmware is a CrossChess build too, use\n"
    "the same shortcut on it).\n"
    "\n"
    "Install a file from this folder: hold Side Up, then press and hold Power\n"
    "until the device starts (Recovery mode). Open crosschess, then backup,\n"
    "and select the file.\n"
    "\n"
    "bootloader-0x0.bin is the bootloader and partition table, for a restore\n"
    "over the serial port. It holds no Wi-Fi passwords.\n"
    "\n"
    "Резервные копии CrossChess\n"
    "\n"
    "Когда новая сборка CrossChess запускается впервые, она копирует сюда\n"
    "прошивку, которую заменила: firmware-<имя>-<версия>-<код>.bin.\n"
    "Каждая копия занимает около 6 МБ, старые можно удалять.\n"
    "\n"
    "Автоматический откат: если новая сборка трижды подряд не смогла\n"
    "запуститься, устройство само вернётся к прежней прошивке. Зависший\n"
    "запуск перезапускается через минуту и считается неудачным.\n"
    "\n"
    "Обе комбинации ниже работают на выключенном устройстве без USB: с\n"
    "питанием от USB оно включается, не проверяя кнопки. На X3 боковая\n"
    "кнопка «вверх» на левом краю, «вниз» на правом.\n"
    "\n"
    "Вернуться вручную: зажмите боковую кнопку «вниз», затем нажмите и\n"
    "держите кнопку питания до включения. Устройство будет запускать прежнюю\n"
    "прошивку, пока вы снова не установите новую сборку (или, если прежняя\n"
    "тоже сборка CrossChess, не повторите на ней ту же комбинацию).\n"
    "\n"
    "Установить файл из этой папки: зажмите боковую кнопку «вверх», затем\n"
    "нажмите и держите кнопку питания до включения (режим восстановления).\n"
    "Откройте папку crosschess, затем backup, и выберите файл.\n"
    "\n"
    "bootloader-0x0.bin — загрузчик и таблица разделов для восстановления\n"
    "через последовательный порт. Паролей Wi-Fi в нём нет.\n";

// Kept in NVS so it survives crashes and power cycles. Describes one build:
// the one running now, or the one a rollback is about to boot.
struct Record {
  uint32_t partitionAddress;
  uint8_t elfSha[8];  // prefix of the build's ELF SHA-256
  uint8_t starts;     // starts since install that neither got healthy nor slept
  uint8_t healthy;
  uint8_t backupDone;
  uint8_t backupTries;
};

Record current{};
bool watching = false;  // waiting to mark the running build healthy

// The RTC watchdog runs off its own clock, so it still resets the chip when
// interrupts are off or every task is stuck. ESP-IDF turns it off once the
// app has started and only takes it over again to finish a panic or an
// esp_restart(); light sleep leaves it alone while it runs.
wdt_hal_context_t watchdog{};
bool watchdogRunning = false;

void startWatchdog() {
  const uint32_t ticks = static_cast<uint32_t>(uint64_t{kWatchdogMs} * rtc_clk_slow_freq_get_hz() / 1000);
  // The same system reset esp_restart() makes (the RTC domain keeps
  // running), which these boards already go through after every update.
  wdt_hal_init(&watchdog, WDT_RWDT, 0, false);
  wdt_hal_write_protect_disable(&watchdog);
  wdt_hal_config_stage(&watchdog, WDT_STAGE0, ticks, WDT_STAGE_ACTION_RESET_SYSTEM);
  wdt_hal_enable(&watchdog);
  wdt_hal_write_protect_enable(&watchdog);
  watchdogRunning = true;
}

void feedWatchdog() {
  if (!watchdogRunning) return;
  wdt_hal_write_protect_disable(&watchdog);
  wdt_hal_feed(&watchdog);
  wdt_hal_write_protect_enable(&watchdog);
}

Record recordFor(const esp_partition_t* partition, const uint8_t* elfSha) {
  Record record{};
  record.partitionAddress = partition->address;
  memcpy(record.elfSha, elfSha, sizeof(record.elfSha));
  return record;
}

bool sameBuild(const Record& a, const Record& b) {
  return a.partitionAddress == b.partitionAddress && memcmp(a.elfSha, b.elfSha, sizeof(a.elfSha)) == 0;
}

bool loadRecord(Record& record) {
  nvs_handle_t handle;
  if (nvs_open(kNvsNamespace, NVS_READONLY, &handle) != ESP_OK) return false;
  size_t size = sizeof(record);
  const bool ok = nvs_get_blob(handle, kRecordKey, &record, &size) == ESP_OK && size == sizeof(record);
  nvs_close(handle);
  return ok;
}

void saveRecord(const Record& record) {
  nvs_handle_t handle;
  esp_err_t err = nvs_open(kNvsNamespace, NVS_READWRITE, &handle);
  if (err == ESP_OK) {
    err = nvs_set_blob(handle, kRecordKey, &record, sizeof(record));
    if (err == ESP_OK) err = nvs_commit(handle);
    nvs_close(handle);
  }
  if (err != ESP_OK) LOG_ERR("GUARD", "Saving boot record failed: %d", err);
}

// Returns only when there is no previous firmware to boot.
void bootPreviousFirmware(const esp_partition_t* running, const bool rollback) {
  const esp_partition_t* previous = esp_ota_get_next_update_partition(running);
  esp_app_desc_t desc;
  if (!previous || previous->address == running->address ||
      esp_ota_get_partition_description(previous, &desc) != ESP_OK) {
    LOG_ERR("GUARD", "No previous firmware to boot");
    return;
  }

  // After a rollback the previous firmware is the one that installed the
  // failing build, so it got through startup. Recording it as healthy stops
  // its own guard (when it is a CrossChess build too) from bouncing straight
  // back. A switch by hand records nothing: the target may be the very build
  // a rollback left, and its guard must count its starts again.
  if (rollback) {
    Record target = recordFor(previous, desc.app_elf_sha256);
    target.healthy = 1;
    target.backupDone = 1;
    saveRecord(target);
  }

  if (!ota_boot::switchTo(previous)) {
    LOG_ERR("GUARD", "Switching to %s failed", previous->label);
    return;
  }
  LOG_INF("GUARD", "Rebooting into %s (%s)", previous->label, desc.version);
  delay(50);  // let the log line out
  esp_restart();
}

// Length of the app image at the start of `partition`, or 0 if it does not
// parse. Same layout walk as firmware_flash::validateImageFile, so the copy is
// accepted by the SD card firmware update.
size_t imageLength(const esp_partition_t* partition) {
  esp_image_header_t header;
  if (esp_partition_read(partition, 0, &header, sizeof(header)) != ESP_OK || header.magic != ESP_IMAGE_HEADER_MAGIC ||
      header.segment_count > ESP_IMAGE_MAX_SEGMENTS) {
    return 0;
  }
  size_t pos = sizeof(header);
  for (uint8_t i = 0; i < header.segment_count; ++i) {
    esp_image_segment_header_t segment;
    if (esp_partition_read(partition, pos, &segment, sizeof(segment)) != ESP_OK) return 0;
    pos += sizeof(segment) + segment.data_len;
    if (pos > partition->size) return 0;
  }
  // The checksum byte ends the next 16-byte block; an optional SHA-256 follows.
  pos = (pos + 16) & ~static_cast<size_t>(15);
  if (header.hash_appended) pos += 32;
  return pos <= partition->size ? pos : 0;
}

// Copies raw flash to `path`, writing a .part file first so a failed copy
// never leaves a truncated image that looks like a real backup. A .part left
// by a power cut is replaced.
bool copyFlashToFile(uint32_t flashAddress, size_t length, const char* path) {
  const std::unique_ptr<uint8_t[]> buffer(new (std::nothrow) uint8_t[kCopyChunk]);
  if (!buffer) return false;
  char partPath[128];
  snprintf(partPath, sizeof(partPath), "%s.part", path);
  if (Storage.exists(partPath)) Storage.remove(partPath);

  HalFile file;
  if (!Storage.openFileForWrite("GUARD", partPath, file)) return false;
  bool ok = true;
  for (size_t offset = 0; ok && offset < length; offset += kCopyChunk) {
    feedWatchdog();
    const size_t chunk = std::min(kCopyChunk, length - offset);
    ok = esp_flash_read(nullptr, buffer.get(), flashAddress + offset, chunk) == ESP_OK &&
         file.write(buffer.get(), chunk) == chunk;
  }
  ok = file.close() && ok;
  ok = ok && Storage.rename(partPath, path);
  if (!ok) Storage.remove(partPath);
  return ok;
}

bool backupBootRegion() {
  char path[64];
  snprintf(path, sizeof(path), "%s/bootloader-0x0.bin", kBackupDir);
  if (Storage.exists(path)) return true;
  if (copyFlashToFile(0, kBootRegionSize, path)) return true;
  LOG_ERR("GUARD", "Bootloader backup failed");
  return false;
}

// True when the previous firmware is on the card, or there is none to copy.
bool backupPreviousApp() {
  const esp_partition_t* running = esp_ota_get_running_partition();
  const esp_partition_t* previous = esp_ota_get_next_update_partition(running);
  esp_app_desc_t desc;
  if (!previous || previous->address == running->address ||
      esp_ota_get_partition_description(previous, &desc) != ESP_OK) {
    LOG_INF("GUARD", "No previous firmware to back up");
    return true;
  }
  const size_t length = imageLength(previous);
  if (length == 0) {
    LOG_ERR("GUARD", "Previous firmware in %s does not parse, not backing it up", previous->label);
    return true;
  }

  char name[sizeof(desc.project_name) + sizeof(desc.version)];
  snprintf(name, sizeof(name), "%s-%s", desc.project_name, desc.version);
  for (char* c = name; *c; ++c) {
    if (!isalnum(static_cast<unsigned char>(*c)) && *c != '.' && *c != '-') *c = '_';
  }
  // The version alone repeats across builds (every uncommitted build of a
  // commit ends in -dirty), so the ELF hash tells them apart; otherwise the
  // existing file of an older build would count as this one's backup.
  const uint8_t* sha = desc.app_elf_sha256;
  char path[128];
  snprintf(path, sizeof(path), "%s/firmware-%s-%02x%02x%02x%02x.bin", kBackupDir, name, sha[0], sha[1], sha[2], sha[3]);
  if (Storage.exists(path)) return true;

  const unsigned long started = millis();
  if (!copyFlashToFile(previous->address, length, path)) {
    LOG_ERR("GUARD", "Backing up %s failed", previous->label);
    return false;
  }
  LOG_INF("GUARD", "Backed up %s (%u bytes) to %s in %lu ms", previous->label, static_cast<unsigned>(length), path,
          millis() - started);
  return true;
}

}  // namespace

// Runs before Serial is up, so checkSwitchButton() logs the start instead.
// NVS is ready: the Arduino core initialises it before setup().
void begin() {
  const esp_partition_t* running = esp_ota_get_running_partition();
  if (!running) return;

  current = recordFor(running, esp_app_get_description()->app_elf_sha256);
  Record saved;
  if (loadRecord(saved) && sameBuild(saved, current)) current = saved;
  if (current.healthy) return;

  if (current.starts < UINT8_MAX) ++current.starts;
  saveRecord(current);
  if (current.starts > kMaxFailedStarts) {
    LOG_ERR("GUARD", "%u starts without getting healthy, booting the previous firmware", current.starts - 1);
    bootPreviousFirmware(running, true);
  }
  startWatchdog();
  watching = true;
}

void checkSwitchButton(const bool sideDownHeld) {
  if (watching) {
    LOG_INF("GUARD", "Start %u of a build not yet healthy (reset reason %d)", current.starts,
            static_cast<int>(esp_reset_reason()));
  }
  if (!sideDownHeld) return;
  const esp_partition_t* running = esp_ota_get_running_partition();
  if (!running) return;
  LOG_INF("GUARD", "Side Down held at power-on, switching to the previous firmware");
  // A switch by hand is not a failed start of this build, which keeps its
  // record should the same build be installed into this slot again.
  if (watching && current.starts > 0) {
    --current.starts;
    saveRecord(current);
  }
  bootPreviousFirmware(running, false);
}

void backupPreviousFirmware() {
  if (current.partitionAddress == 0 || current.backupDone) return;
  // Count the try before copying: if the copy itself crashed, retrying on
  // every start would end in a rollback.
  if (current.backupTries >= kMaxBackupTries) return;
  ++current.backupTries;
  saveRecord(current);

  if (!Storage.exists(kBackupDir) && !Storage.mkdir(kBackupDir)) {
    LOG_ERR("GUARD", "Cannot create %s", kBackupDir);
    return;
  }
  char readmePath[64];
  snprintf(readmePath, sizeof(readmePath), "%s/README.txt", kBackupDir);
  Storage.writeFile(readmePath, kBackupReadme);
  const bool bootRegion = backupBootRegion();
  if (backupPreviousApp() && bootRegion) {
    current.backupDone = 1;
    saveRecord(current);
  }
}

void loop() {
  if (!watching) return;
  feedWatchdog();
  if (millis() < kHealthyAfterMs) return;
  watching = false;
  stopWatchdog();
  current.healthy = 1;
  current.starts = 0;
  saveRecord(current);
  LOG_INF("GUARD", "Build marked healthy");
}

void sleeping() {
  // The watchdog pauses in sleep, but a wake starts over anyway.
  stopWatchdog();
  if (!watching || current.starts == 0) return;
  --current.starts;
  saveRecord(current);
}

void stopWatchdog() {
  if (!watchdogRunning) return;
  watchdogRunning = false;
  wdt_hal_write_protect_disable(&watchdog);
  wdt_hal_disable(&watchdog);
  wdt_hal_write_protect_enable(&watchdog);
}

}  // namespace crosschess::bootguard

#endif
