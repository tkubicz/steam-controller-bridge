#include <Adafruit_TinyUSB.h>
#include <Arduino.h>
#include <nrf.h>
#include <type_traits>

extern "C" {
#include "device/usbd_pvt.h"
}

#include "src/bridge_protocol.h"
#include "src/bridge_session.h"
#include "src/xinput_gamepad.h"

namespace xinput_usb {

constexpr uint16_t kDescriptorLength = 40;
constexpr uint8_t kInEndpoint = 0x83;
constexpr uint8_t kOutEndpoint = 0x02;

struct State {
  volatile bool opened;
  volatile bool out_needs_arm;
  volatile uint8_t in_endpoint;
  volatile uint8_t out_endpoint;
  uint8_t in_buffer[32] __attribute__((aligned(4)));
  uint8_t out_buffer[32] __attribute__((aligned(4)));
};

State state{};

void driver_init() {
  state.opened = false;
  state.out_needs_arm = false;
  state.in_endpoint = 0;
  state.out_endpoint = 0;
}

void driver_reset(uint8_t) { driver_init(); }

uint16_t driver_open(uint8_t rhport, tusb_desc_interface_t const* interface,
                     uint16_t max_length) {
  if (max_length < kDescriptorLength ||
      interface->bInterfaceClass != 0xff ||
      interface->bInterfaceSubClass != 0x5d ||
      interface->bInterfaceProtocol != 0x01) {
    return 0;
  }

  const uint8_t* bytes = reinterpret_cast<const uint8_t*>(interface);
  const auto* endpoint_in =
      reinterpret_cast<const tusb_desc_endpoint_t*>(bytes + 26);
  const auto* endpoint_out =
      reinterpret_cast<const tusb_desc_endpoint_t*>(bytes + 33);
  if (endpoint_in->bDescriptorType != TUSB_DESC_ENDPOINT ||
      endpoint_out->bDescriptorType != TUSB_DESC_ENDPOINT ||
      !usbd_edpt_open(rhport, endpoint_in) ||
      !usbd_edpt_open(rhport, endpoint_out)) {
    return 0;
  }

  state.in_endpoint = endpoint_in->bEndpointAddress;
  state.out_endpoint = endpoint_out->bEndpointAddress;
  state.out_needs_arm = true;
  state.opened = true;
  return kDescriptorLength;
}

bool driver_control(uint8_t, uint8_t, tusb_control_request_t const*) {
  return false;
}

bool driver_transfer(uint8_t, uint8_t endpoint, xfer_result_t, uint32_t) {
  if (endpoint == state.out_endpoint) {
    state.out_needs_arm = true;
  }
  return true;
}

const usbd_class_driver_t driver = {
#if CFG_TUSB_DEBUG >= 2
    .name = "XINPUT",
#endif
    .init = driver_init,
    .reset = driver_reset,
    .open = driver_open,
    .control_xfer_cb = driver_control,
    .xfer_cb = driver_transfer,
    .sof = nullptr,
};

class Interface final : public Adafruit_USBD_Interface {
 public:
  uint16_t getInterfaceDescriptor(uint8_t interface_number, uint8_t* buffer,
                                  uint16_t buffer_size) override {
    if (buffer == nullptr) {
      return kDescriptorLength;
    }
    if (buffer_size < kDescriptorLength) {
      return 0;
    }

    // The Seeed core installs CDC first (interfaces 0/1, IN endpoints 1/2,
    // OUT endpoint 1). This fixed composite personality therefore assigns
    // XInput interface 2 with IN endpoint 3 and OUT endpoint 2.
    const uint8_t descriptor[kDescriptorLength] = {
        9, TUSB_DESC_INTERFACE, interface_number, 0, 2, 0xff, 0x5d, 0x01, 0,
        17, 0x21, 0x00, 0x01, 0x01, 0x25, kInEndpoint, 0x14, 0x00, 0x00,
        0x00, 0x00, 0x13, kOutEndpoint, 0x08, 0x00, 0x00,
        7, TUSB_DESC_ENDPOINT, kInEndpoint, TUSB_XFER_INTERRUPT,
        U16_TO_U8S_LE(32), 4,
        7, TUSB_DESC_ENDPOINT, kOutEndpoint, TUSB_XFER_INTERRUPT,
        U16_TO_U8S_LE(32), 8,
    };
    memcpy(buffer, descriptor, sizeof(descriptor));
    return sizeof(descriptor);
  }

  bool begin() { return TinyUSBDevice.addInterface(*this); }
};

Interface interface;

void service_out_endpoint() {
  const uint8_t endpoint = state.out_endpoint;
  if (!state.opened || !state.out_needs_arm || endpoint == 0 ||
      usbd_edpt_busy(0, endpoint) || !usbd_edpt_claim(0, endpoint)) {
    return;
  }
  if (usbd_edpt_xfer(0, endpoint, state.out_buffer,
                     sizeof(state.out_buffer))) {
    state.out_needs_arm = false;
  } else {
    usbd_edpt_release(0, endpoint);
  }
}

bool send(const scbridge::XInputGamepadReport& report) {
  const uint8_t endpoint = state.in_endpoint;
  if (!state.opened || endpoint == 0 || !TinyUSBDevice.mounted() ||
      usbd_edpt_busy(0, endpoint) || !usbd_edpt_claim(0, endpoint)) {
    return false;
  }
  memcpy(state.in_buffer, &report, sizeof(report));
  if (usbd_edpt_xfer(0, endpoint, state.in_buffer, sizeof(report))) {
    return true;
  }
  usbd_edpt_release(0, endpoint);
  return false;
}

}  // namespace xinput_usb

extern "C" const usbd_class_driver_t* usbd_app_driver_get_cb(
    uint8_t* driver_count) {
  *driver_count = 1;
  return &xinput_usb::driver;
}

namespace {

constexpr size_t kCdcChunkSize = 64;
constexpr size_t kTxQueueDepth = 4;
constexpr uint32_t kHardwareWatchdogTicks = 2U * 32768U;

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

void service_gamepad() {
  xinput_usb::service_out_endpoint();
  if (!session.hid_report_pending() || !TinyUSBDevice.mounted()) {
    return;
  }
  const scbridge::CanonicalGamepadReport& report =
      session.pending_hid_report();
  const scbridge::XInputGamepadReport usb_report =
      scbridge::make_xinput_report(report);
  if (xinput_usb::send(usb_report)) {
    session.mark_hid_report_sent();
  }
}

}  // namespace

void setup() {
  TinyUSBDevice.setID(0x045e, 0x028e);
  TinyUSBDevice.setDeviceVersion(0x0114);
  TinyUSBDevice.setManufacturerDescriptor("Lynxware");
  TinyUSBDevice.setProductDescriptor("Steam Controller Bridge");

  // Apple's Xbox driver matches both the FF/5D/01 interface and a
  // vendor-class top-level device. The BSP defaults composite devices to the
  // IAD class (EF/02/01), which binds the interface driver but does not publish
  // a connected GCController. Adafruit_USBD_Device is standard-layout and its
  // device descriptor is its first member, so the object and descriptor are
  // pointer-interconvertible here.
  static_assert(std::is_standard_layout<Adafruit_USBD_Device>::value,
                "USB device descriptor access requires standard layout");
  auto* device_descriptor =
      reinterpret_cast<tusb_desc_device_t*>(&TinyUSBDevice);
  device_descriptor->bDeviceClass = 0xff;
  device_descriptor->bDeviceSubClass = 0x00;
  device_descriptor->bDeviceProtocol = 0x00;

  xinput_usb::interface.setStringDescriptor(
      "Steam Controller Bridge Xbox Gamepad");
  xinput_usb::interface.begin();
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
  const uint32_t after_cdc_ms = millis();
  session.tick(after_cdc_ms);
  service_gamepad();
  service_led(after_cdc_ms);
  feed_hardware_watchdog();
}
