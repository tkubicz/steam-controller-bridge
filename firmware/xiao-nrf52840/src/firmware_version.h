#pragma once

#include <stddef.h>
#include <stdint.h>

namespace scbridge {

// Hand-maintained monotonic firmware revision, independent of release
// numbering. Bump it in the same commit as any behavior-affecting firmware
// change; raise the host's MINIMUM_FIRMWARE_REVISION (bridge-output) only
// when the host depends on the new behavior.
constexpr uint16_t kFirmwareRevision = 2;

// DeviceInfo payload: format, revision, capability flags, receipt state, and
// recorded receipt fields when present. A three-byte revision 1 payload stays
// valid for older installed firmware.
constexpr uint8_t kDeviceInfoFormat = 1;
constexpr size_t kDeviceInfoBasePayloadSize = 8;
constexpr size_t kDeviceInfoRecordedPayloadSize = 33;

}  // namespace scbridge
