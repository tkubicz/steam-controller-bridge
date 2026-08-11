#pragma once

#include <stddef.h>
#include <stdint.h>

#include "firmware_version.h"

namespace scbridge {

constexpr size_t kFlashPageSize = 4096;
constexpr uint16_t kInstallReceiptFormat = 1;
constexpr uint32_t kInstallReceiptSlotMagic = 0x31524353U;
constexpr uint32_t kInstallReceiptCommit = 0x54494d43U;
constexpr uint8_t kInstallSourceAppCenter = 1;
constexpr uint8_t kInstallSourceFirstObserved = 2;

struct InstallReceiptData {
  uint64_t installed_at;
  uint8_t install_id[16];
  uint8_t source;

  bool operator==(const InstallReceiptData& other) const;
};

struct InstallReceiptSlot {
  uint32_t magic;
  uint16_t firmware_revision;
  uint8_t source;
  uint8_t reserved0;
  uint64_t installed_at;
  uint8_t install_id[16];
  uint32_t crc32;
  uint32_t commit;
  uint8_t reserved[24];
};

static_assert(sizeof(InstallReceiptSlot) == 64,
              "receipt slot must remain exactly 64 bytes");

struct alignas(kFlashPageSize) InstallReceiptPage {
  uint32_t magic[2];
  uint16_t format;
  uint16_t firmware_revision;
  uint32_t reserved0;
  InstallReceiptSlot slots[2];
  uint8_t reserved[kFlashPageSize - 16 - 2 * sizeof(InstallReceiptSlot)];
};

static_assert(sizeof(InstallReceiptPage) == kFlashPageSize,
              "receipt marker must occupy exactly one flash page");
static_assert(alignof(InstallReceiptPage) == kFlashPageSize,
              "receipt marker must be flash-page aligned");

extern const InstallReceiptPage kInstallReceiptPage;

enum class InstallReceiptState : uint8_t {
  Pending = 1,
  Recorded = 2,
  Invalid = 3,
};

struct InstallReceiptStatus {
  InstallReceiptState state;
  InstallReceiptData receipt;
};

class ReceiptWordWriter {
 public:
  virtual ~ReceiptWordWriter() = default;
  virtual bool write_word(size_t page_offset, uint32_t value) = 0;
};

uint32_t crc32_ieee(const uint8_t* data, size_t length);
bool install_receipt_slot_blank(const InstallReceiptSlot& slot);
bool validate_install_receipt(const InstallReceiptSlot& slot,
                              InstallReceiptData* receipt);
InstallReceiptStatus read_install_receipt(
    const volatile InstallReceiptPage& page);

// Programs a blank slot and commits it last. This function never erases flash.
bool write_install_receipt(const volatile InstallReceiptPage& page,
                           const InstallReceiptData& receipt,
                           ReceiptWordWriter& writer);

}  // namespace scbridge
