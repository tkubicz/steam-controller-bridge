#include <Adafruit_TinyUSB.h>
#include <Arduino.h>
#include <nrf.h>
#include <nrf_sdm.h>
#include <nrf_soc.h>
#include <type_traits>

extern "C" {
#include "device/usbd_pvt.h"
}

#include "src/bridge_protocol.h"
#include "src/bridge_session.h"
#include "src/install_receipt.h"
#include "src/xinput_gamepad.h"

namespace xinput_usb {

constexpr uint16_t kDescriptorLength = 40;
constexpr uint8_t kInEndpoint = 0x83;
constexpr uint8_t kOutEndpoint = 0x02;

struct State {
  volatile bool opened;
  volatile bool out_needs_arm;
  volatile bool output_pending;
  volatile uint8_t output_length;
  volatile uint8_t interface_number;
  volatile uint8_t in_endpoint;
  volatile uint8_t out_endpoint;
  uint8_t in_buffer[32] __attribute__((aligned(4)));
  uint8_t out_buffer[32] __attribute__((aligned(4)));
  uint8_t pending_output[scbridge::kXInputRumbleReportSize]
      __attribute__((aligned(4)));
  uint8_t control_output[scbridge::kXInputRumbleReportSize]
      __attribute__((aligned(4)));
};

State state{};

void driver_init() {
  state.opened = false;
  state.out_needs_arm = false;
  state.output_pending = false;
  state.output_length = 0;
  state.interface_number = 0xff;
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

  state.interface_number = interface->bInterfaceNumber;
  state.in_endpoint = endpoint_in->bEndpointAddress;
  state.out_endpoint = endpoint_out->bEndpointAddress;
  state.out_needs_arm = true;
  state.opened = true;
  return kDescriptorLength;
}

void stage_output(const uint8_t* data, uint32_t length) {
  const uint8_t copy_length =
      length <= sizeof(state.pending_output)
          ? static_cast<uint8_t>(length)
          : static_cast<uint8_t>(sizeof(state.pending_output));
  memcpy(state.pending_output, data, copy_length);
  state.output_length =
      length <= UINT8_MAX ? static_cast<uint8_t>(length) : UINT8_MAX;
  state.output_pending = true;
}

bool driver_control(uint8_t rhport, uint8_t stage,
                    tusb_control_request_t const* request) {
  if (request == nullptr ||
      !scbridge::is_xinput_output_set_report(
          request->bmRequestType_bit.direction == TUSB_DIR_OUT,
          request->bmRequestType_bit.type == TUSB_REQ_TYPE_CLASS,
          request->bmRequestType_bit.recipient == TUSB_REQ_RCPT_INTERFACE,
          request->bRequest, request->wValue, request->wIndex,
          request->wLength, state.interface_number)) {
    return false;
  }
  if (stage == CONTROL_STAGE_SETUP) {
    return tud_control_xfer(rhport, request, state.control_output,
                            sizeof(state.control_output));
  }
  if (stage == CONTROL_STAGE_ACK) {
    stage_output(state.control_output, sizeof(state.control_output));
  }
  return true;
}

bool driver_transfer(uint8_t, uint8_t endpoint, xfer_result_t result,
                     uint32_t transferred_bytes) {
  if (endpoint == state.out_endpoint) {
    if (result == XFER_RESULT_SUCCESS) {
      stage_output(state.out_buffer, transferred_bytes);
    }
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

bool take_output_report(uint8_t* data, size_t capacity, size_t* length) {
  if (!state.output_pending || data == nullptr || length == nullptr) {
    return false;
  }
  const size_t staged_length = state.output_length;
  if (staged_length > capacity) {
    state.output_pending = false;
    state.output_length = 0;
    return false;
  }
  memcpy(data, state.pending_output, staged_length);
  state.output_pending = false;
  state.output_length = 0;
  *length = staged_length;
  return true;
}

void discard_pending_output() {
  state.output_pending = false;
  state.output_length = 0;
}

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
constexpr uint32_t kUf2DrainDelayMs = 100;
constexpr uint32_t kReceiptWriteTimeoutMs = 2000;

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

  bool empty() const { return count_ == 0U; }

  scbridge::InstallReceiptStatus install_receipt() const override {
    return scbridge::read_install_receipt(scbridge::kInstallReceiptPage);
  }

  bool record_install_receipt(
      const scbridge::InstallReceiptData& receipt) override;

 private:
  QueuedFrame queue_[kTxQueueDepth]{};
  size_t head_ = 0;
  size_t tail_ = 0;
  size_t count_ = 0;
};

class FlashReceiptWriter final : public scbridge::ReceiptWordWriter {
 public:
  FlashReceiptWriter() : started_at_(millis()) {}

  bool write_word(size_t page_offset, uint32_t value) override {
    const uintptr_t page =
        reinterpret_cast<uintptr_t>(&scbridge::kInstallReceiptPage);
    // Volatile keeps the flash store ordered against the NVMC register
    // accesses instead of relying on the readback to pin it.
    auto* destination =
        reinterpret_cast<volatile uint32_t*>(page + page_offset);
    const volatile uint32_t* observed = destination;
    if (*observed == value) {
      return true;
    }
    if ((*observed & value) != value) {
      return false;
    }

    uint8_t softdevice_enabled = 0;
    if (sd_softdevice_is_enabled(&softdevice_enabled) != NRF_SUCCESS) {
      return false;
    }
    // This USB-only firmware never enables the SoftDevice. Refuse to program
    // if that invariant changes, because SoftDevice flash writes are
    // asynchronous and require ownership of their completion event.
    if (softdevice_enabled != 0U) {
      return false;
    }

    if (!wait_for_nvmc()) {
      return false;
    }
    NRF_NVMC->CONFIG = NVMC_CONFIG_WEN_Wen << NVMC_CONFIG_WEN_Pos;
    bool written = wait_for_nvmc();
    if (written) {
      *destination = value;
      written = wait_for_nvmc() && *observed == value;
    }
    NRF_NVMC->CONFIG = NVMC_CONFIG_WEN_Ren << NVMC_CONFIG_WEN_Pos;
    return wait_for_nvmc() && written;
  }

 private:
  bool timed_out() const {
    return static_cast<uint32_t>(millis() - started_at_) >=
           kReceiptWriteTimeoutMs;
  }

  bool wait_for_nvmc() const {
    while (NRF_NVMC->READY == NVMC_READY_READY_Busy) {
      if (timed_out()) {
        return false;
      }
      NRF_WDT->RR[0] = WDT_RR_RR_Reload;
    }
    return true;
  }

  uint32_t started_at_;
};

bool UsbCdcQueue::record_install_receipt(
    const scbridge::InstallReceiptData& receipt) {
  FlashReceiptWriter writer;
  return scbridge::write_install_receipt(
      scbridge::kInstallReceiptPage, receipt, writer);
}

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
bool uf2_transition_started = false;
bool uf2_queue_drained = false;
uint32_t uf2_queue_drained_at = 0;

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
    session.on_hid_mounted(now_ms);
  } else if (!mounted && previous_usb_mounted) {
    session.on_cdc_disconnected();
    decoder.reset();
    cdc_tx.clear();
    xinput_usb::discard_pending_output();
  }
  previous_usb_mounted = mounted;

  const bool dtr = mounted && Serial.dtr();
  if (dtr && !previous_dtr) {
    decoder.reset();
    cdc_tx.clear();
    xinput_usb::discard_pending_output();
    session.on_cdc_connected(now_ms);
  } else if (!dtr && previous_dtr) {
    session.on_cdc_disconnected();
    decoder.reset();
    cdc_tx.clear();
    xinput_usb::discard_pending_output();
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

void service_gamepad(uint32_t now_ms) {
  uint8_t output_report[scbridge::kXInputRumbleReportSize];
  size_t output_length = 0;
  if (xinput_usb::take_output_report(output_report, sizeof(output_report),
                                     &output_length)) {
    scbridge::RumbleFeedback rumble{};
    if (scbridge::parse_xinput_rumble(output_report, output_length, &rumble)) {
      session.on_xinput_rumble(rumble, now_ms);
    }
  }
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

void service_uf2_transition(uint32_t now_ms) {
  if (!uf2_transition_started && session.uf2_bootloader_ready()) {
    uf2_transition_started = true;
  }
  if (!uf2_transition_started) {
    return;
  }
  if (!uf2_queue_drained) {
    if (!cdc_tx.empty()) {
      return;
    }
    Serial.flush();
    uf2_queue_drained = true;
    uf2_queue_drained_at = now_ms;
    return;
  }
  if (static_cast<uint32_t>(now_ms - uf2_queue_drained_at) >=
      kUf2DrainDelayMs) {
    enterUf2Dfu();
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
  service_gamepad(after_cdc_ms);
  service_uf2_transition(after_cdc_ms);
  service_led(after_cdc_ms);
  feed_hardware_watchdog();
}
