#pragma once

#include <stddef.h>
#include <stdint.h>

namespace scbridge {

// Hand-maintained monotonic firmware revision, independent of release
// numbering. Bump it in the same commit as any behavior-affecting firmware
// change; raise the host's MINIMUM_FIRMWARE_REVISION (bridge-output) only
// when the host depends on the new behavior.
constexpr uint16_t kFirmwareRevision = 1;

// DeviceInfo payload: format byte, then the revision as u16 little-endian.
// Receivers accept longer payloads and ignore trailing bytes, so fields may
// be appended without a format bump.
constexpr uint8_t kDeviceInfoFormat = 1;
constexpr size_t kDeviceInfoPayloadSize = 3;

}  // namespace scbridge
