#pragma once

#include <stddef.h>
#include <stdint.h>

namespace scbridge {

// Hand-maintained monotonic firmware revision, independent of release
// numbering. Bump it in the same commit as any behavior-affecting firmware
// change. Host update policy is owned by the matching firmware-target catalog
// entry rather than by this board-neutral protocol.
constexpr uint16_t kFirmwareRevision = 3;

// DeviceInfo payload: format, revision, capability flags, receipt state, and
// recorded receipt fields when present. A three-byte revision 1 payload stays
// valid for older installed firmware.
constexpr uint8_t kDeviceInfoFormat = 1;
constexpr size_t kDeviceInfoBasePayloadSize = 8;
constexpr size_t kDeviceInfoRecordedPayloadSize = 33;
constexpr uint8_t kFirmwareTargetTlv = 1;
constexpr char kFirmwareTargetId[] = "seeed-xiao-nrf52840";
constexpr size_t kFirmwareTargetIdSize = sizeof(kFirmwareTargetId) - 1U;
constexpr size_t kFirmwareTargetExtensionSize = 2U + kFirmwareTargetIdSize;
constexpr size_t kDeviceInfoMaximumPayloadSize =
    kDeviceInfoRecordedPayloadSize + kFirmwareTargetExtensionSize;

}  // namespace scbridge
