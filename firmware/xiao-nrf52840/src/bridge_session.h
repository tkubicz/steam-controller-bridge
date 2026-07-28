#pragma once

#include "bridge_protocol.h"

namespace scbridge {

constexpr uint32_t kDataWatchdogMs = 100;
constexpr uint32_t kRumbleLeaseRefreshMs = 25;

struct RumbleFeedback {
  uint16_t low_frequency;
  uint16_t high_frequency;

  bool operator==(const RumbleFeedback& other) const {
    return low_frequency == other.low_frequency &&
           high_frequency == other.high_frequency;
  }
  bool operator!=(const RumbleFeedback& other) const {
    return !(*this == other);
  }
};

#pragma pack(push, 1)
struct CanonicalGamepadReport {
  uint16_t buttons;
  uint8_t hat;
  int16_t left_x;
  int16_t left_y;
  int16_t right_x;
  int16_t right_y;
  uint16_t left_trigger;
  uint16_t right_trigger;
};
#pragma pack(pop)

static_assert(sizeof(CanonicalGamepadReport) == 15,
              "canonical gamepad report must be exactly 15 bytes");

struct SessionDiagnostics {
  uint32_t decode_errors;
  uint32_t sequence_gaps;
  uint32_t watchdog_neutrals;
  uint32_t rumble_feedback_frames;
  uint32_t rumble_feedback_refreshes;
};

class SessionSink {
 public:
  virtual ~SessionSink() = default;
  virtual bool queue_cdc(const uint8_t* data, size_t length) = 0;
};

class BridgeSession {
 public:
  explicit BridgeSession(SessionSink& sink);

  void on_cdc_connected(uint32_t now_ms);
  void on_cdc_disconnected();
  void on_hid_mounted();
  void on_frame(const Frame& frame, uint32_t now_ms);
  void on_decode_error(DecodeError error);
  void on_xinput_rumble(const RumbleFeedback& rumble, uint32_t now_ms);
  void tick(uint32_t now_ms);

  bool negotiated() const { return negotiated_; }
  bool faulted() const { return faulted_; }
  bool cdc_connected() const { return cdc_connected_; }
  const SessionDiagnostics& diagnostics() const { return diagnostics_; }

  bool hid_report_pending() const { return hid_pending_; }
  const CanonicalGamepadReport& pending_hid_report() const {
    return pending_hid_;
  }
  void mark_hid_report_sent();

 private:
  void reset_session(bool keep_connection);
  void check_sequence(uint16_t sequence);
  void apply_gamepad(const Frame& frame, uint32_t now_ms);
  void force_neutral(bool safety);
  void force_rumble_zero();
  void queue_hid(const CanonicalGamepadReport& report, bool safety);
  void queue_rumble(const RumbleFeedback& rumble, bool safety);
  void service_rumble(uint32_t now_ms);
  bool send_message(MessageType type, const uint8_t* payload,
                    uint16_t payload_length);
  static CanonicalGamepadReport neutral_report();
  static RumbleFeedback zero_rumble();
  static bool report_is_neutral(const CanonicalGamepadReport& report);
  static bool rumble_is_active(const RumbleFeedback& rumble);

  SessionSink& sink_;
  bool cdc_connected_;
  bool negotiated_;
  bool sequence_valid_;
  bool faulted_;
  bool data_watchdog_armed_;
  bool hid_pending_;
  bool pending_is_safety_neutral_;
  bool deferred_active_pending_;
  bool rumble_pending_;
  bool rumble_pending_is_safety_zero_;
  bool rumble_pending_is_refresh_;
  bool deferred_rumble_pending_;
  bool rumble_refresh_armed_;
  uint8_t consecutive_errors_;
  uint16_t expected_sequence_;
  uint16_t transmit_sequence_;
  uint32_t last_data_ms_;
  uint32_t last_rumble_tx_ms_;
  CanonicalGamepadReport pending_hid_;
  CanonicalGamepadReport deferred_active_;
  RumbleFeedback desired_rumble_;
  RumbleFeedback pending_rumble_;
  RumbleFeedback deferred_rumble_;
  SessionDiagnostics diagnostics_;
};

}  // namespace scbridge
