#include "install_receipt.h"

#include <string.h>

#include <cstddef>

namespace scbridge {
namespace {

constexpr uint32_t kPageMagic0 = 0x5249'4353U;
constexpr uint32_t kPageMagic1 = 0x3130'5043U;

constexpr InstallReceiptSlot make_blank_slot() {
  InstallReceiptSlot slot{};
  slot.magic = UINT32_MAX;
  slot.firmware_revision = UINT16_MAX;
  slot.source = UINT8_MAX;
  slot.reserved0 = UINT8_MAX;
  slot.installed_at = UINT64_MAX;
  for (uint8_t& byte : slot.install_id) {
    byte = UINT8_MAX;
  }
  slot.crc32 = UINT32_MAX;
  slot.commit = UINT32_MAX;
  for (uint8_t& byte : slot.reserved) {
    byte = UINT8_MAX;
  }
  return slot;
}

constexpr InstallReceiptPage make_blank_page() {
  InstallReceiptPage page{};
  page.magic[0] = kPageMagic0;
  page.magic[1] = kPageMagic1;
  page.format = kInstallReceiptFormat;
  page.firmware_revision = kFirmwareRevision;
  page.reserved0 = UINT32_MAX;
  page.slots[0] = make_blank_slot();
  page.slots[1] = make_blank_slot();
  for (uint8_t& byte : page.reserved) {
    byte = UINT8_MAX;
  }
  return page;
}

bool valid_source(uint8_t source) {
  return source == kInstallSourceAppCenter ||
         source == kInstallSourceFirstObserved;
}

bool install_id_present(const uint8_t* install_id) {
  uint8_t combined = 0;
  for (size_t i = 0; i < 16; ++i) {
    combined = static_cast<uint8_t>(combined | install_id[i]);
  }
  return combined != 0;
}

uint32_t load_word(const uint8_t* bytes) {
  uint32_t value = 0;
  memcpy(&value, bytes, sizeof(value));
  return value;
}

InstallReceiptSlot copy_slot(
    const volatile InstallReceiptSlot& source) {
  InstallReceiptSlot slot;
  const volatile auto* source_bytes =
      reinterpret_cast<const volatile uint8_t*>(&source);
  auto* destination = reinterpret_cast<uint8_t*>(&slot);
  for (size_t index = 0; index < sizeof(slot); ++index) {
    destination[index] = source_bytes[index];
  }
  return slot;
}

}  // namespace

#if defined(__APPLE__)
#define SCBRIDGE_RECEIPT_SECTION "__DATA,__receipt"
#else
#define SCBRIDGE_RECEIPT_SECTION ".install_receipt"
#endif

const InstallReceiptPage kInstallReceiptPage
    __attribute__((used, section(SCBRIDGE_RECEIPT_SECTION))) =
        make_blank_page();

bool InstallReceiptData::operator==(const InstallReceiptData& other) const {
  return installed_at == other.installed_at && source == other.source &&
         memcmp(install_id, other.install_id, sizeof(install_id)) == 0;
}

uint32_t crc32_ieee(const uint8_t* data, size_t length) {
  uint32_t crc = 0xffff'ffffU;
  for (size_t i = 0; i < length; ++i) {
    crc ^= data[i];
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc & 1U) != 0U ? (crc >> 1U) ^ 0xedb8'8320U : crc >> 1U;
    }
  }
  return crc ^ 0xffff'ffffU;
}

bool install_receipt_slot_blank(const InstallReceiptSlot& slot) {
  const auto* bytes = reinterpret_cast<const uint8_t*>(&slot);
  for (size_t i = 0; i < sizeof(slot); ++i) {
    if (bytes[i] != 0xffU) {
      return false;
    }
  }
  return true;
}

bool validate_install_receipt(const InstallReceiptSlot& slot,
                              InstallReceiptData* receipt) {
  if (slot.magic != kInstallReceiptSlotMagic ||
      slot.firmware_revision != kFirmwareRevision ||
      !valid_source(slot.source) || slot.installed_at == 0 ||
      slot.installed_at > INT64_MAX || !install_id_present(slot.install_id) ||
      slot.commit != kInstallReceiptCommit) {
    return false;
  }
  const auto* bytes = reinterpret_cast<const uint8_t*>(&slot);
  if (crc32_ieee(bytes, offsetof(InstallReceiptSlot, crc32)) != slot.crc32) {
    return false;
  }
  if (receipt != nullptr) {
    receipt->installed_at = slot.installed_at;
    memcpy(receipt->install_id, slot.install_id,
           sizeof(receipt->install_id));
    receipt->source = slot.source;
  }
  return true;
}

InstallReceiptStatus read_install_receipt(
    const volatile InstallReceiptPage& page) {
  InstallReceiptStatus status{};
  status.state = InstallReceiptState::Invalid;
  if (page.magic[0] != kPageMagic0 || page.magic[1] != kPageMagic1 ||
      page.format != kInstallReceiptFormat ||
      page.firmware_revision != kFirmwareRevision) {
    return status;
  }
  for (size_t index = 0; index < 2; ++index) {
    const InstallReceiptSlot slot = copy_slot(page.slots[index]);
    if (validate_install_receipt(slot, &status.receipt)) {
      status.state = InstallReceiptState::Recorded;
      return status;
    }
  }
  for (size_t index = 0; index < 2; ++index) {
    const InstallReceiptSlot slot = copy_slot(page.slots[index]);
    if (install_receipt_slot_blank(slot)) {
      status.state = InstallReceiptState::Pending;
      return status;
    }
  }
  return status;
}

bool write_install_receipt(const volatile InstallReceiptPage& page,
                           const InstallReceiptData& receipt,
                           ReceiptWordWriter& writer) {
  if (read_install_receipt(page).state != InstallReceiptState::Pending ||
      receipt.installed_at == 0 || receipt.installed_at > INT64_MAX ||
      !valid_source(receipt.source) ||
      !install_id_present(receipt.install_id)) {
    return false;
  }

  size_t slot_index = 0;
  while (slot_index < 2 && !install_receipt_slot_blank(
                               copy_slot(page.slots[slot_index]))) {
    ++slot_index;
  }
  if (slot_index == 2) {
    return false;
  }

  InstallReceiptSlot slot;
  memset(&slot, 0xff, sizeof(slot));
  slot.magic = kInstallReceiptSlotMagic;
  slot.firmware_revision = kFirmwareRevision;
  slot.source = receipt.source;
  slot.reserved0 = 0;
  slot.installed_at = receipt.installed_at;
  memcpy(slot.install_id, receipt.install_id, sizeof(slot.install_id));
  slot.crc32 = crc32_ieee(reinterpret_cast<const uint8_t*>(&slot),
                          offsetof(InstallReceiptSlot, crc32));

  const size_t base = offsetof(InstallReceiptPage, slots) +
                      slot_index * sizeof(InstallReceiptSlot);
  const auto* bytes = reinterpret_cast<const uint8_t*>(&slot);
  for (size_t offset = 0; offset < offsetof(InstallReceiptSlot, commit);
       offset += sizeof(uint32_t)) {
    if (!writer.write_word(base + offset, load_word(bytes + offset))) {
      return false;
    }
  }
  if (!writer.write_word(base + offsetof(InstallReceiptSlot, commit),
                         kInstallReceiptCommit)) {
    return false;
  }
  const InstallReceiptSlot recorded = copy_slot(page.slots[slot_index]);
  return validate_install_receipt(recorded, nullptr);
}

}  // namespace scbridge
