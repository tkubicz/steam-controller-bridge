#pragma once

#include <stddef.h>

#include "bridge_session.h"

namespace scbridge {

#pragma pack(push, 1)
struct XInputGamepadReport {
  uint8_t message_type;
  uint8_t message_size;
  uint16_t buttons;
  uint8_t left_trigger;
  uint8_t right_trigger;
  int16_t left_x;
  int16_t left_y;
  int16_t right_x;
  int16_t right_y;
  uint8_t reserved[6];
};
#pragma pack(pop)

static_assert(sizeof(XInputGamepadReport) == 20,
              "XInput report must be exactly 20 bytes");

constexpr size_t kXInputRumbleReportSize = 8;

inline bool is_xinput_output_set_report(
    bool direction_out, bool class_request, bool interface_recipient,
    uint8_t request, uint16_t value, uint16_t index, uint16_t length,
    uint8_t interface_number) {
  return direction_out && class_request && interface_recipient &&
         request == 0x09U && value == 0x0200U &&
         index == interface_number && length == kXInputRumbleReportSize;
}

inline bool parse_xinput_rumble(const uint8_t* data, size_t length,
                                RumbleFeedback* output) {
  if (data == nullptr || output == nullptr ||
      length != kXInputRumbleReportSize || data[0] != 0x00U ||
      data[1] != kXInputRumbleReportSize || data[2] != 0x00U ||
      data[5] != 0x00U || data[6] != 0x00U || data[7] != 0x00U) {
    return false;
  }
  output->low_frequency =
      static_cast<uint16_t>(static_cast<uint16_t>(data[3]) * 257U);
  output->high_frequency =
      static_cast<uint16_t>(static_cast<uint16_t>(data[4]) * 257U);
  return true;
}

constexpr uint16_t source_button(uint8_t index) {
  return static_cast<uint16_t>(1U << index);
}

inline uint8_t xinput_trigger(uint16_t value) {
  return static_cast<uint8_t>(static_cast<uint32_t>(value) / 257U);
}

inline XInputGamepadReport make_xinput_report(
    const CanonicalGamepadReport& source) {
  XInputGamepadReport report{};
  report.message_size = sizeof(XInputGamepadReport);

  switch (source.hat) {
    case 0:
      report.buttons |= 0x0001;
      break;
    case 1:
      report.buttons |= 0x0001 | 0x0008;
      break;
    case 2:
      report.buttons |= 0x0008;
      break;
    case 3:
      report.buttons |= 0x0002 | 0x0008;
      break;
    case 4:
      report.buttons |= 0x0002;
      break;
    case 5:
      report.buttons |= 0x0002 | 0x0004;
      break;
    case 6:
      report.buttons |= 0x0004;
      break;
    case 7:
      report.buttons |= 0x0001 | 0x0004;
      break;
    default:
      break;
  }

  if (source.buttons & source_button(9)) report.buttons |= 0x0010;   // Start
  if (source.buttons & source_button(8)) report.buttons |= 0x0020;   // Back
  if (source.buttons & source_button(6)) report.buttons |= 0x0040;   // L3
  if (source.buttons & source_button(7)) report.buttons |= 0x0080;   // R3
  if (source.buttons & source_button(4)) report.buttons |= 0x0100;   // LB
  if (source.buttons & source_button(5)) report.buttons |= 0x0200;   // RB
  if (source.buttons & source_button(10)) report.buttons |= 0x0400;  // Guide
  if (source.buttons & source_button(0)) report.buttons |= 0x1000;   // A
  if (source.buttons & source_button(1)) report.buttons |= 0x2000;   // B
  if (source.buttons & source_button(2)) report.buttons |= 0x4000;   // X
  if (source.buttons & source_button(3)) report.buttons |= 0x8000;   // Y

  report.left_trigger = xinput_trigger(source.left_trigger);
  report.right_trigger = xinput_trigger(source.right_trigger);
  report.left_x = source.left_x;
  report.left_y = source.left_y;
  report.right_x = source.right_x;
  report.right_y = source.right_y;
  return report;
}

}  // namespace scbridge
