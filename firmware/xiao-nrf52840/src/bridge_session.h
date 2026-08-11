#pragma once

#include "bridge_protocol.h"
#include "install_receipt.h"

namespace scbridge {

constexpr uint32_t kDataWatchdogMs = 100;
constexpr uint32_t kRumbleLeaseRefreshMs = 25;
constexpr uint32_t kEnterUf2BootloaderCapability = 1U << 0U;
constexpr uint32_t kInstallReceiptCapability = 1U << 1U;
constexpr uint32_t kFirmwareCapabilities =
    kEnterUf2BootloaderCapability | kInstallReceiptCapability;

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

  bool operator==(const CanonicalGamepadReport& other) const {
    return buttons == other.buttons && hat == other.hat &&
           left_x == other.left_x && left_y == other.left_y &&
           right_x == other.right_x && right_y == other.right_y &&
           left_trigger == other.left_trigger &&
           right_trigger == other.right_trigger;
  }
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
  uint32_t suppressed_hid_duplicates;
};

class SessionSink {
 public:
  virtual ~SessionSink() = default;
  virtual bool queue_cdc(const uint8_t* data, size_t length) = 0;
  virtual InstallReceiptStatus install_receipt() const = 0;
  virtual bool record_install_receipt(const InstallReceiptData& receipt) = 0;
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
  bool uf2_bootloader_ready() const { return uf2_bootloader_ready_; }
  uint32_t uf2_bootloader_request_id() const {
    return uf2_bootloader_request_id_;
  }
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
  void service_device_info();
  void service_rumble(uint32_t now_ms);
  void service_install_receipt();
  void service_uf2_bootloader();
  void begin_uf2_bootloader(const Frame& frame);
  void record_install_receipt(const Frame& frame);
  bool send_install_receipt_recorded(uint32_t request_id,
                                     const InstallReceiptData& receipt);
  bool send_error(ControlErrorCode code, uint32_t request_id);
  bool send_message(MessageType type, const uint8_t* payload,
                    uint16_t payload_length);
  static CanonicalGamepadReport neutral_report();
  static RumbleFeedback zero_rumble();
  static bool report_is_neutral(const CanonicalGamepadReport& report);
  static bool rumble_is_active(const RumbleFeedback& rumble);

  SessionSink& sink_;
  bool cdc_connected_;
  bool negotiated_;
  bool device_info_pending_;
  bool sequence_valid_;
  bool faulted_;
  bool data_watchdog_armed_;
  bool hid_pending_;
  bool pending_is_safety_neutral_;
  bool deferred_active_pending_;
  bool last_queued_hid_valid_;
  bool rumble_pending_;
  bool rumble_pending_is_safety_zero_;
  bool rumble_pending_is_refresh_;
  bool deferred_rumble_pending_;
  bool rumble_refresh_armed_;
  bool uf2_bootloader_requested_;
  bool uf2_bootloader_ready_pending_;
  bool uf2_bootloader_ready_;
  bool install_receipt_requested_;
  uint8_t consecutive_errors_;
  uint16_t expected_sequence_;
  uint16_t transmit_sequence_;
  uint32_t last_data_ms_;
  uint32_t last_rumble_tx_ms_;
  uint32_t uf2_bootloader_request_id_;
  uint32_t install_receipt_request_id_;
  InstallReceiptData requested_install_receipt_;
  CanonicalGamepadReport pending_hid_;
  CanonicalGamepadReport deferred_active_;
  CanonicalGamepadReport last_queued_hid_;
  RumbleFeedback desired_rumble_;
  RumbleFeedback pending_rumble_;
  RumbleFeedback deferred_rumble_;
  SessionDiagnostics diagnostics_;
};

}  // namespace scbridge
