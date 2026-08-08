#include "bridge_session.h"

#include <string.h>

#include "firmware_version.h"

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
      device_info_pending_(false),
      sequence_valid_(false),
      faulted_(false),
      data_watchdog_armed_(false),
      hid_pending_(true),
      pending_is_safety_neutral_(true),
      deferred_active_pending_(false),
      last_queued_hid_valid_(false),
      rumble_pending_(true),
      rumble_pending_is_safety_zero_(true),
      rumble_pending_is_refresh_(false),
      deferred_rumble_pending_(false),
      rumble_refresh_armed_(false),
      consecutive_errors_(0),
      expected_sequence_(0),
      transmit_sequence_(0),
      last_data_ms_(0),
      last_rumble_tx_ms_(0),
      pending_hid_(neutral_report()),
      deferred_active_(neutral_report()),
      last_queued_hid_(neutral_report()),
      desired_rumble_(zero_rumble()),
      pending_rumble_(zero_rumble()),
      deferred_rumble_(zero_rumble()),
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

void BridgeSession::on_hid_mounted() {
  // A freshly (re)mounted USB host has accepted no report, so the queue cache
  // no longer describes its endpoint and the neutral below must reach the wire
  // for the driver to publish the controller.
  last_queued_hid_valid_ = false;
  force_neutral(true);
  force_rumble_zero();
}

void BridgeSession::on_frame(const Frame& frame, uint32_t now_ms) {
  consecutive_errors_ = 0;
  faulted_ = false;

  if (frame.message_type == static_cast<uint8_t>(MessageType::Hello)) {
    force_neutral(true);
    force_rumble_zero();
    negotiated_ = false;
    sequence_valid_ = true;
    expected_sequence_ = static_cast<uint16_t>(frame.sequence + 1U);
    if (frame.payload[0] <= kProtocolVersion &&
        frame.payload[1] >= kProtocolVersion) {
      const uint8_t selected = kProtocolVersion;
      negotiated_ =
          send_message(MessageType::HelloResponse, &selected, 1);
    }
    // Queued behind the HelloResponse rather than sent inline: the CDC TX
    // queue is shallow, and the loop's tick retries until it accepts.
    device_info_pending_ = negotiated_;
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
    case MessageType::Rumble:
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
    force_rumble_zero();
  }
}

void BridgeSession::on_xinput_rumble(const RumbleFeedback& rumble,
                                     uint32_t) {
  if (!negotiated_) {
    return;
  }
  queue_rumble(rumble, false);
}

void BridgeSession::tick(uint32_t now_ms) {
  if (data_watchdog_armed_ &&
      static_cast<uint32_t>(now_ms - last_data_ms_) >= kDataWatchdogMs) {
    data_watchdog_armed_ = false;
    faulted_ = true;
    ++diagnostics_.watchdog_neutrals;
    force_neutral(true);
    force_rumble_zero();
  }
  service_device_info();
  service_rumble(now_ms);
}

void BridgeSession::mark_hid_report_sent() {
  if (!hid_pending_) {
    return;
  }
  // xinput_usb::send succeeding means TinyUSB accepted the endpoint transfer,
  // not that the host has already polled it. A USB reset/remount invalidates
  // this cache and unconditionally queues a fresh neutral baseline.
  last_queued_hid_ = pending_hid_;
  last_queued_hid_valid_ = true;
  const bool was_safety_neutral = pending_is_safety_neutral_;
  hid_pending_ = false;
  pending_is_safety_neutral_ = false;
  if (was_safety_neutral && deferred_active_pending_) {
    const CanonicalGamepadReport deferred = deferred_active_;
    deferred_active_pending_ = false;
    queue_hid(deferred, false);
  }
}

void BridgeSession::reset_session(bool keep_connection) {
  negotiated_ = false;
  device_info_pending_ = false;
  sequence_valid_ = false;
  consecutive_errors_ = 0;
  faulted_ = false;
  data_watchdog_armed_ = false;
  deferred_active_pending_ = false;
  force_rumble_zero();
  if (!keep_connection) {
    transmit_sequence_ = 0;
  }
  force_neutral(true);
}

void BridgeSession::check_sequence(uint16_t sequence) {
  if (sequence_valid_ && sequence != expected_sequence_) {
    ++diagnostics_.sequence_gaps;
    force_neutral(true);
    force_rumble_zero();
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

void BridgeSession::force_rumble_zero() {
  rumble_refresh_armed_ = false;
  queue_rumble(zero_rumble(), true);
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
  if (last_queued_hid_valid_ && report == last_queued_hid_) {
    // The USB endpoint has already accepted this report, so nothing needs the
    // wire; this also cancels an older unsent change the report reverts.
    // Resending an identical neutral would register as gamepad input on macOS
    // and abort the very sleep a CDC teardown belongs to.
    hid_pending_ = false;
    pending_is_safety_neutral_ = false;
    ++diagnostics_.suppressed_hid_duplicates;
    return;
  }
  pending_hid_ = report;
  hid_pending_ = true;
  pending_is_safety_neutral_ = safety;
}

void BridgeSession::service_device_info() {
  if (!negotiated_ || !device_info_pending_) {
    return;
  }
  uint8_t payload[kDeviceInfoPayloadSize];
  payload[0] = kDeviceInfoFormat;
  payload[1] = static_cast<uint8_t>(kFirmwareRevision);
  payload[2] = static_cast<uint8_t>(kFirmwareRevision >> 8U);
  if (send_message(MessageType::DeviceInfo, payload, sizeof(payload))) {
    device_info_pending_ = false;
  }
}

void BridgeSession::queue_rumble(const RumbleFeedback& rumble, bool safety) {
  desired_rumble_ = rumble;
  rumble_refresh_armed_ = rumble_is_active(rumble);
  if (rumble_pending_is_safety_zero_ && !safety &&
      rumble_is_active(rumble)) {
    deferred_rumble_ = rumble;
    deferred_rumble_pending_ = true;
    return;
  }
  if (safety) {
    deferred_rumble_pending_ = false;
  }
  pending_rumble_ = rumble;
  rumble_pending_ = true;
  rumble_pending_is_safety_zero_ = safety;
  rumble_pending_is_refresh_ = false;
}

void BridgeSession::service_rumble(uint32_t now_ms) {
  if (!negotiated_) {
    return;
  }
  if (!rumble_pending_ && rumble_refresh_armed_ &&
      static_cast<uint32_t>(now_ms - last_rumble_tx_ms_) >=
          kRumbleLeaseRefreshMs) {
    pending_rumble_ = desired_rumble_;
    rumble_pending_ = true;
    rumble_pending_is_safety_zero_ = false;
    rumble_pending_is_refresh_ = true;
  }
  if (!rumble_pending_) {
    return;
  }
  uint8_t payload[kRumblePayloadSize];
  payload[0] = static_cast<uint8_t>(pending_rumble_.low_frequency);
  payload[1] =
      static_cast<uint8_t>(pending_rumble_.low_frequency >> 8U);
  payload[2] = static_cast<uint8_t>(pending_rumble_.high_frequency);
  payload[3] =
      static_cast<uint8_t>(pending_rumble_.high_frequency >> 8U);
  if (!send_message(MessageType::Rumble, payload, sizeof(payload))) {
    return;
  }
  last_rumble_tx_ms_ = now_ms;
  ++diagnostics_.rumble_feedback_frames;
  if (rumble_pending_is_refresh_) {
    ++diagnostics_.rumble_feedback_refreshes;
  }
  if (rumble_pending_is_safety_zero_ && deferred_rumble_pending_) {
    pending_rumble_ = deferred_rumble_;
    rumble_pending_is_safety_zero_ = false;
    rumble_pending_is_refresh_ = false;
    deferred_rumble_pending_ = false;
    rumble_pending_ = true;
    return;
  }
  rumble_pending_ = false;
  rumble_pending_is_safety_zero_ = false;
  rumble_pending_is_refresh_ = false;
}

bool BridgeSession::send_message(MessageType type, const uint8_t* payload,
                                 uint16_t payload_length) {
  uint8_t frame[kMaxFrameSize];
  const size_t length =
      encode_frame(transmit_sequence_, static_cast<uint8_t>(type), payload,
                   payload_length, frame, sizeof(frame));
  if (length != 0U && sink_.queue_cdc(frame, length)) {
    transmit_sequence_ = static_cast<uint16_t>(transmit_sequence_ + 1U);
    return true;
  }
  return false;
}

CanonicalGamepadReport BridgeSession::neutral_report() {
  CanonicalGamepadReport report{};
  report.hat = 8;
  return report;
}

RumbleFeedback BridgeSession::zero_rumble() {
  return RumbleFeedback{0, 0};
}

bool BridgeSession::report_is_neutral(
    const CanonicalGamepadReport& report) {
  const CanonicalGamepadReport neutral = neutral_report();
  return report == neutral;
}

bool BridgeSession::rumble_is_active(const RumbleFeedback& rumble) {
  return rumble.low_frequency != 0U || rumble.high_frequency != 0U;
}

}  // namespace scbridge
