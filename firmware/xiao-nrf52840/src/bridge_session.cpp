#include "bridge_session.h"

#include <string.h>

namespace scbridge {
namespace {

uint16_t read_u16(const uint8_t* data) {
  return static_cast<uint16_t>(data[0]) |
         static_cast<uint16_t>(static_cast<uint16_t>(data[1]) << 8U);
}

int16_t read_i16(const uint8_t* data) {
  return static_cast<int16_t>(read_u16(data));
}

}  // namespace

BridgeSession::BridgeSession(SessionSink& sink)
    : sink_(sink),
      cdc_connected_(false),
      negotiated_(false),
      sequence_valid_(false),
      faulted_(false),
      data_watchdog_armed_(false),
      hid_pending_(true),
      pending_is_safety_neutral_(true),
      deferred_active_pending_(false),
      consecutive_errors_(0),
      expected_sequence_(0),
      transmit_sequence_(0),
      last_data_ms_(0),
      pending_hid_(neutral_report()),
      deferred_active_(neutral_report()),
      diagnostics_{} {}

void BridgeSession::on_cdc_connected(uint32_t now_ms) {
  cdc_connected_ = true;
  last_data_ms_ = now_ms;
  reset_session(true);
}

void BridgeSession::on_cdc_disconnected() {
  cdc_connected_ = false;
  reset_session(false);
}

void BridgeSession::on_hid_mounted() { force_neutral(true); }

void BridgeSession::on_frame(const Frame& frame, uint32_t now_ms) {
  consecutive_errors_ = 0;
  faulted_ = false;

  if (frame.message_type == static_cast<uint8_t>(MessageType::Hello)) {
    force_neutral(true);
    negotiated_ = false;
    sequence_valid_ = true;
    expected_sequence_ = static_cast<uint16_t>(frame.sequence + 1U);
    if (frame.payload[0] <= kProtocolVersion &&
        frame.payload[1] >= kProtocolVersion) {
      const uint8_t selected = kProtocolVersion;
      send_message(MessageType::HelloResponse, &selected, 1);
      negotiated_ = true;
    }
    return;
  }

  if (!negotiated_) {
    return;
  }
  check_sequence(frame.sequence);

  switch (static_cast<MessageType>(frame.message_type)) {
    case MessageType::GamepadState:
      apply_gamepad(frame, now_ms);
      break;
    case MessageType::Neutral:
      last_data_ms_ = now_ms;
      data_watchdog_armed_ = false;
      force_neutral(false);
      break;
    case MessageType::Ping:
      send_message(MessageType::Pong, frame.payload, 4);
      break;
    case MessageType::Pong:
    case MessageType::HelloResponse:
    case MessageType::DeviceInfo:
    case MessageType::Error:
    case MessageType::Hello:
      break;
    default:
      break;
  }
}

void BridgeSession::on_decode_error(DecodeError) {
  ++diagnostics_.decode_errors;
  if (consecutive_errors_ < UINT8_MAX) {
    ++consecutive_errors_;
  }
  if (consecutive_errors_ >= 3U) {
    faulted_ = true;
    negotiated_ = false;
    sequence_valid_ = false;
    force_neutral(true);
  }
}

void BridgeSession::tick(uint32_t now_ms) {
  if (data_watchdog_armed_ &&
      static_cast<uint32_t>(now_ms - last_data_ms_) >= kDataWatchdogMs) {
    data_watchdog_armed_ = false;
    faulted_ = true;
    ++diagnostics_.watchdog_neutrals;
    force_neutral(true);
  }
}

void BridgeSession::mark_hid_report_sent() {
  if (pending_is_safety_neutral_ && deferred_active_pending_) {
    pending_hid_ = deferred_active_;
    pending_is_safety_neutral_ = false;
    deferred_active_pending_ = false;
    hid_pending_ = true;
    return;
  }
  hid_pending_ = false;
  pending_is_safety_neutral_ = false;
}

void BridgeSession::reset_session(bool keep_connection) {
  negotiated_ = false;
  sequence_valid_ = false;
  consecutive_errors_ = 0;
  faulted_ = false;
  data_watchdog_armed_ = false;
  deferred_active_pending_ = false;
  if (!keep_connection) {
    transmit_sequence_ = 0;
  }
  force_neutral(true);
}

void BridgeSession::check_sequence(uint16_t sequence) {
  if (sequence_valid_ && sequence != expected_sequence_) {
    ++diagnostics_.sequence_gaps;
    force_neutral(true);
  }
  sequence_valid_ = true;
  expected_sequence_ = static_cast<uint16_t>(sequence + 1U);
}

void BridgeSession::apply_gamepad(const Frame& frame, uint32_t now_ms) {
  CanonicalGamepadReport report{};
  report.buttons = static_cast<uint16_t>(read_u16(frame.payload) & 0xffffU);
  report.hat = frame.payload[4];
  report.left_x = read_i16(frame.payload + 6);
  report.left_y = read_i16(frame.payload + 8);
  report.right_x = read_i16(frame.payload + 10);
  report.right_y = read_i16(frame.payload + 12);
  report.left_trigger = read_u16(frame.payload + 14);
  report.right_trigger = read_u16(frame.payload + 16);
  last_data_ms_ = now_ms;
  data_watchdog_armed_ = !report_is_neutral(report);
  queue_hid(report, false);
}

void BridgeSession::force_neutral(bool safety) {
  data_watchdog_armed_ = false;
  queue_hid(neutral_report(), safety);
}

void BridgeSession::queue_hid(const CanonicalGamepadReport& report,
                              bool safety) {
  if (pending_is_safety_neutral_ && !safety) {
    deferred_active_ = report;
    deferred_active_pending_ = true;
    return;
  }
  if (safety) {
    deferred_active_pending_ = false;
  }
  pending_hid_ = report;
  hid_pending_ = true;
  pending_is_safety_neutral_ = safety;
}

void BridgeSession::send_message(MessageType type, const uint8_t* payload,
                                 uint16_t payload_length) {
  uint8_t frame[kMaxFrameSize];
  const size_t length =
      encode_frame(transmit_sequence_, static_cast<uint8_t>(type), payload,
                   payload_length, frame, sizeof(frame));
  if (length != 0U && sink_.queue_cdc(frame, length)) {
    transmit_sequence_ = static_cast<uint16_t>(transmit_sequence_ + 1U);
  }
}

CanonicalGamepadReport BridgeSession::neutral_report() {
  CanonicalGamepadReport report{};
  report.hat = 8;
  return report;
}

bool BridgeSession::report_is_neutral(
    const CanonicalGamepadReport& report) {
  const CanonicalGamepadReport neutral = neutral_report();
  return memcmp(&report, &neutral, sizeof(report)) == 0;
}

}  // namespace scbridge
