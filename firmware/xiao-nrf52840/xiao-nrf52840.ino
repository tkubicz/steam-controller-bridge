#include <Adafruit_TinyUSB.h>
#include <Arduino.h>
#include <nrf.h>

#include "src/bridge_protocol.h"
#include "src/bridge_session.h"

namespace {

constexpr size_t kCdcChunkSize = 64;
constexpr size_t kTxQueueDepth = 4;
constexpr uint32_t kHardwareWatchdogTicks = 2U * 32768U;

// Independently authored generic-gamepad descriptor for the packed 15-byte
// report declared in bridge_session.h.
const uint8_t kHidReportDescriptor[] = {
    0x05, 0x01,        // Usage Page (Generic Desktop)
    0x09, 0x05,        // Usage (Game Pad)
    0xa1, 0x01,        // Collection (Application)
    0x85, 0x01,        //   Report ID (1)
    0x05, 0x09,        //   Usage Page (Button)
    0x19, 0x01,        //   Usage Minimum (1)
    0x29, 0x10,        //   Usage Maximum (16)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x01,        //   Logical Maximum (1)
    0x75, 0x01,        //   Report Size (1)
    0x95, 0x10,        //   Report Count (16)
    0x81, 0x02,        //   Input (Data, Variable, Absolute)
    0x05, 0x01,        //   Usage Page (Generic Desktop)
    0x09, 0x39,        //   Usage (Hat Switch)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x07,        //   Logical Maximum (7)
    0x35, 0x00,        //   Physical Minimum (0)
    0x46, 0x3b, 0x01,  //   Physical Maximum (315)
    0x65, 0x14,        //   Unit (degrees)
    0x75, 0x04,        //   Report Size (4)
    0x95, 0x01,        //   Report Count (1)
    0x81, 0x42,        //   Input (Data, Variable, Absolute, Null)
    0x65, 0x00,        //   Unit (None)
    0x75, 0x04,        //   Report Size (4)
    0x95, 0x01,        //   Report Count (1)
    0x81, 0x03,        //   Input (Constant, Variable, Absolute)
    0x09, 0x30,        //   Usage (X)
    0x09, 0x31,        //   Usage (Y)
    0x09, 0x33,        //   Usage (Rx)
    0x09, 0x34,        //   Usage (Ry)
    0x16, 0x01, 0x80,  //   Logical Minimum (-32767)
    0x26, 0xff, 0x7f,  //   Logical Maximum (32767)
    0x75, 0x10,        //   Report Size (16)
    0x95, 0x04,        //   Report Count (4)
    0x81, 0x02,        //   Input (Data, Variable, Absolute)
    0x09, 0x32,        //   Usage (Z)
    0x09, 0x35,        //   Usage (Rz)
    0x15, 0x00,        //   Logical Minimum (0)
    0x27, 0xff, 0xff, 0x00, 0x00,  // Logical Maximum (65535)
    0x75, 0x10,        //   Report Size (16)
    0x95, 0x02,        //   Report Count (2)
    0x81, 0x02,        //   Input (Data, Variable, Absolute)
    0xc0               // End Collection
};

Adafruit_USBD_HID usb_hid;

struct QueuedFrame {
  size_t length;
  uint8_t data[scbridge::kMaxFrameSize];
};

class UsbCdcQueue final : public scbridge::SessionSink {
 public:
  bool queue_cdc(const uint8_t* data, size_t length) override {
    if (length > sizeof(queue_[0].data) || count_ == kTxQueueDepth) {
      return false;
    }
    QueuedFrame& frame = queue_[tail_];
    frame.length = length;
    memcpy(frame.data, data, length);
    tail_ = (tail_ + 1U) % kTxQueueDepth;
    ++count_;
    return true;
  }

  void service() {
    if (count_ == 0U || !Serial.dtr()) {
      return;
    }
    const QueuedFrame& frame = queue_[head_];
    if (Serial.availableForWrite() < static_cast<int>(frame.length)) {
      return;
    }
    if (Serial.write(frame.data, frame.length) == frame.length) {
      head_ = (head_ + 1U) % kTxQueueDepth;
      --count_;
    }
  }

  void clear() {
    head_ = 0;
    tail_ = 0;
    count_ = 0;
  }

 private:
  QueuedFrame queue_[kTxQueueDepth]{};
  size_t head_ = 0;
  size_t tail_ = 0;
  size_t count_ = 0;
};

UsbCdcQueue cdc_tx;
scbridge::BridgeSession session(cdc_tx);

void decoded_frame(void*, const scbridge::Frame& frame) {
  session.on_frame(frame, millis());
}

void decode_error(void*, scbridge::DecodeError error) {
  session.on_decode_error(error);
}

scbridge::StreamDecoder decoder(decoded_frame, decode_error, nullptr);
bool previous_usb_mounted = false;
bool previous_dtr = false;

void set_led(bool red, bool green, bool blue) {
#if defined(LED_RED) && defined(LED_GREEN) && defined(LED_BLUE)
  digitalWrite(LED_RED, red ? LOW : HIGH);
  digitalWrite(LED_GREEN, green ? LOW : HIGH);
  digitalWrite(LED_BLUE, blue ? LOW : HIGH);
#else
  (void)red;
  (void)green;
  (void)blue;
#endif
}

void service_led(uint32_t now_ms) {
  if (session.faulted()) {
    set_led(((now_ms / 125U) & 1U) == 0U, false, false);
  } else if (!session.cdc_connected()) {
    set_led(false, false, ((now_ms / 500U) & 1U) == 0U);
  } else if (!session.negotiated()) {
    set_led(false, false, ((now_ms / 125U) & 1U) == 0U);
  } else {
    set_led(false, true, false);
  }
}

void start_hardware_watchdog() {
  NRF_WDT->CONFIG =
      (WDT_CONFIG_SLEEP_Run << WDT_CONFIG_SLEEP_Pos) |
      (WDT_CONFIG_HALT_Pause << WDT_CONFIG_HALT_Pos);
  NRF_WDT->CRV = kHardwareWatchdogTicks;
  NRF_WDT->RREN = WDT_RREN_RR0_Msk;
  NRF_WDT->TASKS_START = 1;
}

void feed_hardware_watchdog() { NRF_WDT->RR[0] = WDT_RR_RR_Reload; }

void service_connection(uint32_t now_ms) {
  const bool mounted = TinyUSBDevice.mounted();
  if (mounted && !previous_usb_mounted) {
    session.on_hid_mounted();
  } else if (!mounted && previous_usb_mounted) {
    session.on_cdc_disconnected();
    decoder.reset();
    cdc_tx.clear();
  }
  previous_usb_mounted = mounted;

  const bool dtr = mounted && Serial.dtr();
  if (dtr && !previous_dtr) {
    decoder.reset();
    cdc_tx.clear();
    session.on_cdc_connected(now_ms);
  } else if (!dtr && previous_dtr) {
    session.on_cdc_disconnected();
    decoder.reset();
    cdc_tx.clear();
  }
  previous_dtr = dtr;
}

void service_cdc() {
  if (!previous_dtr) {
    return;
  }
  uint8_t bytes[kCdcChunkSize];
  const int available = Serial.available();
  if (available > 0) {
    const size_t requested =
        static_cast<size_t>(available) < sizeof(bytes)
            ? static_cast<size_t>(available)
            : sizeof(bytes);
    const size_t count = Serial.readBytes(bytes, requested);
    decoder.push(bytes, count);
  }
  cdc_tx.service();
}

void service_hid() {
  if (!session.hid_report_pending() || !TinyUSBDevice.mounted() ||
      !usb_hid.ready()) {
    return;
  }
  const scbridge::HidGamepadReport& report = session.pending_hid_report();
  if (usb_hid.sendReport(scbridge::kHidReportId, &report, sizeof(report))) {
    session.mark_hid_report_sent();
  }
}

}  // namespace

void setup() {
  TinyUSBDevice.setProductDescriptor("Steam Controller Bridge");
  usb_hid.setPollInterval(1);
  usb_hid.setReportDescriptor(kHidReportDescriptor,
                              sizeof(kHidReportDescriptor));
  usb_hid.setStringDescriptor("Steam Controller Bridge Gamepad");
  usb_hid.begin();
  Serial.begin(115200);

#if defined(LED_RED) && defined(LED_GREEN) && defined(LED_BLUE)
  pinMode(LED_RED, OUTPUT);
  pinMode(LED_GREEN, OUTPUT);
  pinMode(LED_BLUE, OUTPUT);
#endif
  set_led(false, false, false);
  start_hardware_watchdog();

  if (TinyUSBDevice.mounted()) {
    TinyUSBDevice.detach();
    delay(10);
    TinyUSBDevice.attach();
  }
}

void loop() {
#ifdef TINYUSB_NEED_POLLING_TASK
  TinyUSBDevice.task();
#endif
  const uint32_t now_ms = millis();
  service_connection(now_ms);
  service_cdc();
  session.tick(now_ms);
  service_hid();
  service_led(now_ms);
  feed_hardware_watchdog();
}
