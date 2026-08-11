#include "bridge_protocol.h"
#include "bridge_session.h"
#include "firmware_version.h"
#include "install_receipt.h"
#include "xinput_gamepad.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#include <vector>

using scbridge::BridgeSession;
using scbridge::CanonicalGamepadReport;
using scbridge::DecodeError;
using scbridge::Frame;
using scbridge::MessageType;
using scbridge::RumbleFeedback;
using scbridge::SessionSink;
using scbridge::StreamDecoder;
using scbridge::XInputGamepadReport;

namespace {

struct DecoderEvents {
  std::vector<Frame> frames;
  std::vector<DecodeError> errors;
};

void collect_frame(void* context, const Frame& frame) {
  static_cast<DecoderEvents*>(context)->frames.push_back(frame);
}

void collect_error(void* context, DecodeError error) {
  static_cast<DecoderEvents*>(context)->errors.push_back(error);
}

std::vector<uint8_t> encode(uint16_t sequence, MessageType type,
                            const std::vector<uint8_t>& payload = {}) {
  uint8_t bytes[scbridge::kMaxFrameSize];
  const size_t length = scbridge::encode_frame(
      sequence, static_cast<uint8_t>(type), payload.data(),
      static_cast<uint16_t>(payload.size()), bytes, sizeof(bytes));
  assert(length != 0);
  return std::vector<uint8_t>(bytes, bytes + length);
}

std::vector<uint8_t> gamepad_payload(uint16_t buttons = 1, int16_t x = 1234) {
  std::vector<uint8_t> payload(18, 0);
  payload[0] = static_cast<uint8_t>(buttons);
  payload[1] = static_cast<uint8_t>(buttons >> 8U);
  payload[4] = 8;
  payload[6] = static_cast<uint8_t>(x);
  payload[7] = static_cast<uint8_t>(static_cast<uint16_t>(x) >> 8U);
  return payload;
}

class CapturingSink final : public SessionSink {
 public:
  CapturingSink() : receipt_page(scbridge::kInstallReceiptPage) {}

  bool queue_cdc(const uint8_t* data, size_t length) override {
    if (reject_next_write) {
      reject_next_write = false;
      return false;
    }
    writes.emplace_back(data, data + length);
    return true;
  }

  scbridge::InstallReceiptStatus install_receipt() const override {
    return scbridge::read_install_receipt(receipt_page);
  }

  bool record_install_receipt(
      const scbridge::InstallReceiptData& receipt) override {
    class MemoryWriter final : public scbridge::ReceiptWordWriter {
     public:
      explicit MemoryWriter(scbridge::InstallReceiptPage& page)
          : page_(page) {}

      bool write_word(size_t page_offset, uint32_t value) override {
        write_offsets.push_back(page_offset);
        memcpy(reinterpret_cast<uint8_t*>(&page_) + page_offset, &value,
               sizeof(value));
        return true;
      }

      scbridge::InstallReceiptPage& page_;
      std::vector<size_t> write_offsets;
    } writer(receipt_page);
    const bool recorded = scbridge::write_install_receipt(
        receipt_page, receipt, writer);
    receipt_write_offsets = writer.write_offsets;
    return recorded;
  }

  bool reject_next_write = false;
  std::vector<std::vector<uint8_t>> writes;
  scbridge::InstallReceiptPage receipt_page;
  std::vector<size_t> receipt_write_offsets;
};

Frame decode_single(const std::vector<uint8_t>& bytes) {
  DecoderEvents events;
  StreamDecoder decoder(collect_frame, collect_error, &events);
  decoder.push(bytes.data(), bytes.size());
  assert(events.errors.empty());
  assert(events.frames.size() == 1);
  return events.frames[0];
}

void negotiate(BridgeSession& session, uint16_t sequence = 0) {
  Frame hello{};
  hello.version = 1;
  hello.message_type = static_cast<uint8_t>(MessageType::Hello);
  hello.sequence = sequence;
  hello.payload_length = 2;
  hello.payload[0] = 1;
  hello.payload[1] = 1;
  session.on_frame(hello, 0);
  assert(session.negotiated());
  session.mark_hid_report_sent();
}

Frame state_frame(uint16_t sequence, uint16_t buttons = 1, int16_t x = 1234) {
  Frame frame{};
  frame.version = 1;
  frame.message_type = static_cast<uint8_t>(MessageType::GamepadState);
  frame.sequence = sequence;
  frame.payload_length = 18;
  const auto payload = gamepad_payload(buttons, x);
  memcpy(frame.payload, payload.data(), payload.size());
  return frame;
}

Frame control_frame(uint16_t sequence, MessageType type, uint32_t request_id) {
  Frame frame{};
  frame.version = 1;
  frame.message_type = static_cast<uint8_t>(type);
  frame.sequence = sequence;
  frame.payload_length = scbridge::kRequestIdPayloadSize;
  for (size_t i = 0; i < 4; ++i) {
    frame.payload[i] = static_cast<uint8_t>(request_id >> (8U * i));
  }
  return frame;
}

class InterruptingWriter final : public scbridge::ReceiptWordWriter {
 public:
  InterruptingWriter(scbridge::InstallReceiptPage& page,
                     size_t successful_writes)
      : page_(page), successful_writes_(successful_writes) {}

  bool write_word(size_t page_offset, uint32_t value) override {
    offsets.push_back(page_offset);
    if (write_count_++ == successful_writes_) {
      return false;
    }
    memcpy(reinterpret_cast<uint8_t*>(&page_) + page_offset, &value,
           sizeof(value));
    return true;
  }

  scbridge::InstallReceiptPage& page_;
  size_t successful_writes_;
  size_t write_count_ = 0;
  std::vector<size_t> offsets;
};

scbridge::InstallReceiptData example_receipt(uint8_t id = 0x42) {
  scbridge::InstallReceiptData receipt{};
  receipt.installed_at = 1'786'456'920;
  memset(receipt.install_id, id, sizeof(receipt.install_id));
  receipt.source = scbridge::kInstallSourceAppCenter;
  return receipt;
}

void test_crc_and_neutral_vector() {
  const uint8_t check[] = {'1', '2', '3', '4', '5', '6', '7', '8', '9'};
  assert(scbridge::crc16_ccitt_false(check, sizeof(check)) == 0x29b1);
  const auto neutral = encode(0, MessageType::Neutral);
  const uint8_t expected[] = {0x53, 0x43, 0x01, 0x04, 0x00,
                              0x00, 0x00, 0x00, 0xe7, 0xfb};
  assert(neutral.size() == sizeof(expected));
  assert(memcmp(neutral.data(), expected, sizeof(expected)) == 0);
}

void test_xinput_report_conversion() {
  CanonicalGamepadReport neutral{};
  neutral.hat = 8;
  const XInputGamepadReport neutral_report =
      scbridge::make_xinput_report(neutral);
  assert(neutral_report.message_type == 0);
  assert(neutral_report.message_size == 20);
  assert(neutral_report.buttons == 0);
  assert(neutral_report.left_x == 0);
  assert(neutral_report.left_y == 0);
  assert(neutral_report.right_x == 0);
  assert(neutral_report.right_y == 0);
  assert(neutral_report.left_trigger == 0);
  assert(neutral_report.right_trigger == 0);

  CanonicalGamepadReport extremes{};
  extremes.buttons = 0xffff;
  extremes.hat = 7;
  extremes.left_x = -32767;
  extremes.left_y = 32767;
  extremes.right_x = 0;
  extremes.right_y = -1;
  extremes.left_trigger = 0x7fff;
  extremes.right_trigger = 0x8000;
  const XInputGamepadReport report =
      scbridge::make_xinput_report(extremes);
  assert(report.buttons == 0xf7f5);
  assert(report.left_x == -32767);
  assert(report.left_y == 32767);
  assert(report.right_x == 0);
  assert(report.right_y == -1);
  assert(report.left_trigger == 127);
  assert(report.right_trigger == 127);

  struct ButtonCase {
    uint8_t source_index;
    uint16_t xinput_mask;
  };
  const ButtonCase button_cases[] = {
      {0, 0x1000},  // A / south
      {1, 0x2000},  // B / east
      {2, 0x4000},  // X / west
      {3, 0x8000},  // Y / north
      {4, 0x0100},  // left shoulder
      {5, 0x0200},  // right shoulder
      {6, 0x0040},  // left stick
      {7, 0x0080},  // right stick
      {8, 0x0020},  // Back
      {9, 0x0010},  // Start
      {10, 0x0400}, // Guide
  };
  for (const ButtonCase& button_case : button_cases) {
    CanonicalGamepadReport source{};
    source.hat = 8;
    source.buttons =
        static_cast<uint16_t>(1U << button_case.source_index);
    assert(scbridge::make_xinput_report(source).buttons ==
           button_case.xinput_mask);
  }

  const uint16_t dpad_cases[] = {
      0x0001,          // north
      0x0001 | 0x0008, // north-east
      0x0008,          // east
      0x0002 | 0x0008, // south-east
      0x0002,          // south
      0x0002 | 0x0004, // south-west
      0x0004,          // west
      0x0001 | 0x0004, // north-west
      0x0000,          // centered
  };
  for (uint8_t hat = 0; hat <= 8; ++hat) {
    CanonicalGamepadReport source{};
    source.hat = hat;
    assert(scbridge::make_xinput_report(source).buttons ==
           dpad_cases[hat]);
  }

  CanonicalGamepadReport unsupported{};
  unsupported.hat = 8;
  unsupported.buttons = 0xf800;
  assert(scbridge::make_xinput_report(unsupported).buttons == 0);
}

void test_xinput_rumble_parser() {
  assert(scbridge::is_xinput_output_set_report(
      true, true, true, 0x09, 0x0200, 2, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      false, true, true, 0x09, 0x0200, 2, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      true, false, true, 0x09, 0x0200, 2, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      true, true, false, 0x09, 0x0200, 2, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      true, true, true, 0x08, 0x0200, 2, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      true, true, true, 0x09, 0x0100, 2, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      true, true, true, 0x09, 0x0200, 3, 8, 2));
  assert(!scbridge::is_xinput_output_set_report(
      true, true, true, 0x09, 0x0200, 2, 9, 2));

  RumbleFeedback rumble{};
  const uint8_t both[] = {0x00, 0x08, 0x00, 0x01,
                          0xff, 0x00, 0x00, 0x00};
  assert(scbridge::parse_xinput_rumble(both, sizeof(both), &rumble));
  assert(rumble.low_frequency == 257);
  assert(rumble.high_frequency == 65535);

  const uint8_t zero[] = {0x00, 0x08, 0x00, 0x00,
                          0x00, 0x00, 0x00, 0x00};
  assert(scbridge::parse_xinput_rumble(zero, sizeof(zero), &rumble));
  assert(rumble.low_frequency == 0);
  assert(rumble.high_frequency == 0);

  uint8_t malformed[sizeof(both)];
  memcpy(malformed, both, sizeof(both));
  malformed[1] = 7;
  assert(!scbridge::parse_xinput_rumble(malformed, sizeof(malformed),
                                        &rumble));
  memcpy(malformed, both, sizeof(both));
  malformed[2] = 1;
  assert(!scbridge::parse_xinput_rumble(malformed, sizeof(malformed),
                                        &rumble));
  memcpy(malformed, both, sizeof(both));
  malformed[7] = 1;
  assert(!scbridge::parse_xinput_rumble(malformed, sizeof(malformed),
                                        &rumble));
  assert(!scbridge::parse_xinput_rumble(both, sizeof(both) - 1, &rumble));
  assert(!scbridge::parse_xinput_rumble(nullptr, sizeof(both), &rumble));
  assert(!scbridge::parse_xinput_rumble(both, sizeof(both), nullptr));
}

void test_stream_recovery_and_splits() {
  const auto first = encode(1, MessageType::Neutral);
  const auto second = encode(2, MessageType::Ping, {1, 2, 3, 4});
  std::vector<uint8_t> stream = {0xaa, 0x53, 0x00};
  stream.insert(stream.end(), first.begin(), first.end());
  stream.insert(stream.end(), second.begin(), second.end());
  for (size_t split = 0; split <= stream.size(); ++split) {
    DecoderEvents events;
    StreamDecoder decoder(collect_frame, collect_error, &events);
    decoder.push(stream.data(), split);
    decoder.push(stream.data() + split, stream.size() - split);
    assert(events.frames.size() == 2);
    assert(events.frames[0].sequence == 1);
    assert(events.frames[1].sequence == 2);
  }

  auto corrupt = first;
  corrupt.back() ^= 0xff;
  corrupt.insert(corrupt.end(), second.begin(), second.end());
  DecoderEvents events;
  StreamDecoder decoder(collect_frame, collect_error, &events);
  decoder.push(corrupt.data(), corrupt.size());
  assert(events.errors.size() == 1);
  assert(events.errors[0] == DecodeError::ChecksumMismatch);
  assert(events.frames.size() == 1);
  assert(events.frames[0].sequence == 2);
}

void test_decoder_validation_and_unknown_messages() {
  auto invalid_hat = encode(3, MessageType::GamepadState, gamepad_payload());
  invalid_hat[12] = 9;  // Payload hat at frame offset 8 + 4.
  const uint16_t crc = scbridge::crc16_ccitt_false(invalid_hat.data(), invalid_hat.size() - 2);
  invalid_hat[invalid_hat.size() - 2] = static_cast<uint8_t>(crc);
  invalid_hat.back() = static_cast<uint8_t>(crc >> 8U);

  DecoderEvents events;
  StreamDecoder decoder(collect_frame, collect_error, &events);
  decoder.push(invalid_hat.data(), invalid_hat.size());
  const auto unknown = encode(4, static_cast<MessageType>(42), {7});
  decoder.push(unknown.data(), unknown.size());
  assert(events.errors.size() == 1);
  assert(events.errors[0] == DecodeError::InvalidHat);
  assert(events.frames.size() == 1);
  assert(events.frames[0].message_type == 42);
}

void test_decoder_rejects_header_and_payload_errors_then_recovers() {
  const auto neutral = encode(9, MessageType::Neutral);

  DecoderEvents trailing_events;
  StreamDecoder trailing_decoder(collect_frame, collect_error, &trailing_events);
  const uint8_t garbage_and_magic[] = {0xaa, 0x53};
  trailing_decoder.push(garbage_and_magic, sizeof(garbage_and_magic));
  trailing_decoder.push(neutral.data() + 1, neutral.size() - 1);
  assert(trailing_events.frames.size() == 1);

  auto wrong_version = neutral;
  wrong_version[2] = 2;
  uint16_t crc =
      scbridge::crc16_ccitt_false(wrong_version.data(), wrong_version.size() - 2);
  wrong_version[wrong_version.size() - 2] = static_cast<uint8_t>(crc);
  wrong_version.back() = static_cast<uint8_t>(crc >> 8U);

  const auto bad_length = encode(10, MessageType::Hello, {1});
  const auto bad_rumble_length = encode(12, MessageType::Rumble, {1, 2, 3});
  auto reserved_axis = encode(11, MessageType::GamepadState, gamepad_payload());
  reserved_axis[14] = 0;
  reserved_axis[15] = 0x80;
  crc = scbridge::crc16_ccitt_false(reserved_axis.data(), reserved_axis.size() - 2);
  reserved_axis[reserved_axis.size() - 2] = static_cast<uint8_t>(crc);
  reserved_axis.back() = static_cast<uint8_t>(crc >> 8U);

  const uint8_t oversized_header[] = {0x53, 0x43, 0x01, 0x03,
                                      0x01, 0x01, 0x00, 0x00};
  std::vector<uint8_t> stream(oversized_header,
                              oversized_header + sizeof(oversized_header));
  stream.insert(stream.end(), wrong_version.begin(), wrong_version.end());
  stream.insert(stream.end(), bad_length.begin(), bad_length.end());
  stream.insert(stream.end(), bad_rumble_length.begin(),
                bad_rumble_length.end());
  stream.insert(stream.end(), reserved_axis.begin(), reserved_axis.end());
  stream.insert(stream.end(), neutral.begin(), neutral.end());

  DecoderEvents events;
  StreamDecoder decoder(collect_frame, collect_error, &events);
  decoder.push(stream.data(), stream.size());
  assert(events.errors.size() == 5);
  assert(events.errors[0] == DecodeError::PayloadTooLarge);
  assert(events.errors[1] == DecodeError::UnsupportedVersion);
  assert(events.errors[2] == DecodeError::InvalidPayloadLength);
  assert(events.errors[3] == DecodeError::InvalidPayloadLength);
  assert(events.errors[4] == DecodeError::ReservedAxisValue);
  assert(events.frames.size() == 1);
  assert(events.frames[0].sequence == 9);
}

void test_install_receipt_validation_recovery_and_commit_order() {
  scbridge::InstallReceiptPage page = scbridge::kInstallReceiptPage;
  assert(page.reserved0 == UINT32_MAX);
  for (uint8_t byte : page.reserved) {
    assert(byte == UINT8_MAX);
  }
  assert(scbridge::read_install_receipt(page).state ==
         scbridge::InstallReceiptState::Pending);

  InterruptingWriter interrupted(page, 3);
  assert(!scbridge::write_install_receipt(page, example_receipt(),
                                          interrupted));
  assert(scbridge::read_install_receipt(page).state ==
         scbridge::InstallReceiptState::Pending);

  const scbridge::InstallReceiptData recovered = example_receipt(0x43);
  InterruptingWriter complete(page, SIZE_MAX);
  assert(scbridge::write_install_receipt(page, recovered, complete));
  assert(!complete.offsets.empty());
  assert(complete.offsets.back() ==
         offsetof(scbridge::InstallReceiptPage, slots) +
             sizeof(scbridge::InstallReceiptSlot) +
             offsetof(scbridge::InstallReceiptSlot, commit));
  const scbridge::InstallReceiptStatus status =
      scbridge::read_install_receipt(page);
  assert(status.state == scbridge::InstallReceiptState::Recorded);
  assert(status.receipt == recovered);
  assert(!scbridge::write_install_receipt(page, example_receipt(0x44),
                                          complete));

  scbridge::InstallReceiptPage reflashed = scbridge::kInstallReceiptPage;
  assert(scbridge::read_install_receipt(reflashed).state ==
         scbridge::InstallReceiptState::Pending);
  reflashed.magic[0] ^= 1;
  assert(scbridge::read_install_receipt(reflashed).state ==
         scbridge::InstallReceiptState::Invalid);

  scbridge::InstallReceiptPage corrupt = scbridge::kInstallReceiptPage;
  memset(&corrupt.slots[0], 0, sizeof(corrupt.slots[0]));
  memset(&corrupt.slots[1], 0, sizeof(corrupt.slots[1]));
  assert(scbridge::read_install_receipt(corrupt).state ==
         scbridge::InstallReceiptState::Invalid);
}

void test_uf2_transition_neutralizes_before_correlated_ready() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session, 0);
  session.on_frame(state_frame(1, 7, 1000), 1);
  session.mark_hid_report_sent();
  session.on_xinput_rumble(RumbleFeedback{0xffff, 0xaaaa}, 1);

  const Frame enter =
      control_frame(2, MessageType::EnterUf2Bootloader, 0xaabb'ccdd);
  session.on_frame(enter, 2);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  session.tick(2);
  assert(!session.uf2_bootloader_ready());
  const Frame zero_rumble = decode_single(sink.writes.back());
  assert(zero_rumble.message_type ==
         static_cast<uint8_t>(MessageType::Rumble));
  assert(zero_rumble.payload[0] == 0 && zero_rumble.payload[1] == 0);
  assert(zero_rumble.payload[2] == 0 && zero_rumble.payload[3] == 0);

  session.mark_hid_report_sent();
  session.tick(3);
  assert(session.uf2_bootloader_ready());
  assert(session.uf2_bootloader_request_id() == 0xaabb'ccdd);
  const Frame ready = decode_single(sink.writes.back());
  assert(ready.message_type ==
         static_cast<uint8_t>(MessageType::Uf2BootloaderReady));
  assert(ready.payload[0] == 0xdd && ready.payload[1] == 0xcc &&
         ready.payload[2] == 0xbb && ready.payload[3] == 0xaa);

  const size_t before_repeat = sink.writes.size();
  const Frame repeated =
      control_frame(3, MessageType::EnterUf2Bootloader, 0xaabb'ccdd);
  session.on_frame(repeated, 4);
  session.tick(4);
  assert(sink.writes.size() == before_repeat + 1);
  assert(decode_single(sink.writes.back()).message_type ==
         static_cast<uint8_t>(MessageType::Uf2BootloaderReady));

  const Frame different =
      control_frame(4, MessageType::EnterUf2Bootloader, 99);
  session.on_frame(different, 5);
  assert(decode_single(sink.writes.back()).message_type ==
         static_cast<uint8_t>(MessageType::Error));
  session.on_frame(state_frame(5, 12, 2000), 6);
  assert(!session.hid_report_pending());
}

void test_receipt_command_records_and_acknowledges_exact_data() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);

  session.on_frame(state_frame(1, 7, 1000), 1);
  assert(session.hid_report_pending());
  session.mark_hid_report_sent();

  const scbridge::InstallReceiptData receipt = example_receipt();
  Frame record{};
  record.version = 1;
  record.message_type =
      static_cast<uint8_t>(MessageType::RecordInstallReceipt);
  record.sequence = 2;
  record.payload_length = scbridge::kInstallReceiptPayloadSize;
  record.payload[0] = 7;
  for (size_t i = 0; i < 8; ++i) {
    record.payload[4 + i] =
        static_cast<uint8_t>(receipt.installed_at >> (8U * i));
  }
  memcpy(record.payload + 12, receipt.install_id,
         sizeof(receipt.install_id));
  record.payload[28] = receipt.source;
  session.on_frame(record, 2);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  assert(sink.install_receipt().state ==
         scbridge::InstallReceiptState::Pending);
  session.mark_hid_report_sent();
  session.tick(3);

  const Frame acknowledged = decode_single(sink.writes.back());
  assert(acknowledged.message_type ==
         static_cast<uint8_t>(MessageType::InstallReceiptRecorded));
  assert(acknowledged.payload_length ==
         scbridge::kInstallReceiptPayloadSize);
  assert(memcmp(acknowledged.payload, record.payload,
                scbridge::kInstallReceiptPayloadSize) == 0);
  assert(sink.install_receipt().state ==
         scbridge::InstallReceiptState::Recorded);
  assert(!sink.receipt_write_offsets.empty());
  assert(sink.receipt_write_offsets.back() ==
             offsetof(scbridge::InstallReceiptPage, slots) +
             offsetof(scbridge::InstallReceiptSlot, commit));

  const std::vector<size_t> first_write_offsets =
      sink.receipt_write_offsets;
  record.payload[0] = 8;
  record.sequence = 3;
  session.on_frame(record, 4);
  const Frame repeated = decode_single(sink.writes.back());
  assert(repeated.message_type ==
         static_cast<uint8_t>(MessageType::InstallReceiptRecorded));
  assert(memcmp(repeated.payload, record.payload,
                scbridge::kInstallReceiptPayloadSize) == 0);
  assert(sink.receipt_write_offsets == first_write_offsets);
}

void test_session_negotiation_sequence_and_watchdog() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session, 0xffff);
  assert(sink.writes.size() == 1);

  session.on_frame(state_frame(0, 3, 2000), 10);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 3);
  session.mark_hid_report_sent();

  session.tick(109);
  assert(!session.hid_report_pending());
  session.tick(110);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  assert(session.pending_hid_report().hat == 8);
  assert(session.diagnostics().watchdog_neutrals == 1);
  session.mark_hid_report_sent();

  // A gap while the delivered view is already neutral forces no duplicate
  // neutral onto the wire; the frame's own state goes straight out.
  session.on_frame(state_frame(5, 9, 3000), 120);
  assert(session.diagnostics().sequence_gaps == 1);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 9);
  session.mark_hid_report_sent();

  // A gap while the delivered view is active keeps neutral-before-active:
  // the safety neutral transmits first, the frame's state follows.
  session.on_frame(state_frame(9, 7, 1000), 121);
  assert(session.diagnostics().sequence_gaps == 2);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  assert(session.pending_hid_report().hat == 8);
  session.mark_hid_report_sent();
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 7);
  session.mark_hid_report_sent();

  Frame ping{};
  ping.version = 1;
  ping.message_type = static_cast<uint8_t>(MessageType::Ping);
  ping.sequence = 10;
  ping.payload_length = 4;
  ping.payload[0] = 0x78;
  ping.payload[1] = 0x56;
  ping.payload[2] = 0x34;
  ping.payload[3] = 0x12;
  session.on_frame(ping, 122);
  const Frame pong = decode_single(sink.writes.back());
  assert(pong.message_type ==
         static_cast<uint8_t>(MessageType::Pong));
  assert(memcmp(pong.payload, ping.payload, 4) == 0);
}

void test_rumble_latest_refresh_and_safety_zero() {
  CapturingSink sink;
  BridgeSession session(sink);

  session.on_xinput_rumble(RumbleFeedback{0xffff, 0xffff}, 0);
  session.tick(0);
  assert(sink.writes.empty());

  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);
  session.on_xinput_rumble(RumbleFeedback{0x1111, 0x2222}, 0);
  session.on_xinput_rumble(RumbleFeedback{0x1234, 0xabcd}, 0);

  session.tick(0);
  Frame frame = decode_single(sink.writes.back());
  assert(frame.message_type == static_cast<uint8_t>(MessageType::Rumble));
  assert(frame.payload_length == 4);
  assert(frame.payload[0] == 0 && frame.payload[1] == 0);
  assert(frame.payload[2] == 0 && frame.payload[3] == 0);

  session.tick(1);
  frame = decode_single(sink.writes.back());
  assert(frame.payload[0] == 0x34 && frame.payload[1] == 0x12);
  assert(frame.payload[2] == 0xcd && frame.payload[3] == 0xab);
  const size_t writes_after_change = sink.writes.size();
  session.tick(25);
  assert(sink.writes.size() == writes_after_change);
  session.tick(26);
  assert(sink.writes.size() == writes_after_change + 1);
  assert(session.diagnostics().rumble_feedback_refreshes == 1);

  session.on_xinput_rumble(RumbleFeedback{0, 0}, 27);
  session.tick(27);
  frame = decode_single(sink.writes.back());
  assert(frame.payload[0] == 0 && frame.payload[1] == 0);
  assert(frame.payload[2] == 0 && frame.payload[3] == 0);
  const size_t writes_after_zero = sink.writes.size();
  session.tick(100);
  assert(sink.writes.size() == writes_after_zero);

  session.on_xinput_rumble(RumbleFeedback{0xffff, 0x0101}, 101);
  sink.reject_next_write = true;
  session.tick(101);
  const size_t writes_after_rejection = sink.writes.size();
  session.tick(102);
  assert(sink.writes.size() == writes_after_rejection + 1);

  session.on_cdc_disconnected();
  session.on_xinput_rumble(RumbleFeedback{0xffff, 0xffff}, 103);
  session.tick(103);
  assert(sink.writes.size() == writes_after_rejection + 1);
}

void test_fault_and_disconnect_neutralize() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);
  session.on_frame(state_frame(1), 1);
  session.mark_hid_report_sent();
  session.on_decode_error(DecodeError::ChecksumMismatch);
  session.on_decode_error(DecodeError::ChecksumMismatch);
  assert(session.negotiated());
  session.on_decode_error(DecodeError::ChecksumMismatch);
  assert(!session.negotiated());
  assert(session.faulted());
  assert(session.hid_report_pending());
  session.mark_hid_report_sent();
  const uint32_t suppressed_before =
      session.diagnostics().suppressed_hid_duplicates;
  session.on_cdc_disconnected();
  assert(!session.cdc_connected());
  // The disconnect's safety neutral matches the delivered view, so the host
  // sees no gamepad input: closing the CDC port on sleep entry must not
  // wake the machine (the v1.4.0 insomnia regression).
  assert(!session.hid_report_pending());
  assert(session.diagnostics().suppressed_hid_duplicates ==
         suppressed_before + 1);
}

void test_active_cdc_disconnect_queues_safety_neutral() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);
  session.on_frame(state_frame(1, 7, 1500), 1);
  session.mark_hid_report_sent();

  session.on_cdc_disconnected();
  assert(!session.cdc_connected());
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  assert(session.pending_hid_report().hat == 8);
}

void test_identical_refreshes_suppress_hid_but_feed_watchdog() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session, 0xffff);

  session.on_frame(state_frame(0, 3, 2000), 0);
  assert(session.hid_report_pending());
  session.mark_hid_report_sent();

  // The host refreshes an unchanged active state every 25 ms; none of the
  // refreshes may queue USB input, yet each must keep the 100 ms watchdog
  // fed across several nominal periods.
  const uint32_t suppressed_before =
      session.diagnostics().suppressed_hid_duplicates;
  for (uint16_t i = 1; i <= 8; ++i) {
    const uint32_t now = static_cast<uint32_t>(i) * 25U;
    session.on_frame(state_frame(i, 3, 2000), now);
    session.tick(now);
    assert(!session.hid_report_pending());
  }
  assert(session.diagnostics().watchdog_neutrals == 0);
  assert(session.diagnostics().suppressed_hid_duplicates ==
         suppressed_before + 8);

  // Stopping the refreshes still forces the neutral at the deadline.
  session.tick(299);
  assert(!session.hid_report_pending());
  session.tick(300);
  assert(session.diagnostics().watchdog_neutrals == 1);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  assert(session.pending_hid_report().hat == 8);
  session.mark_hid_report_sent();

  // Repeated ordinary neutrals after the delivered neutral stay silent.
  session.on_frame(state_frame(9, 0, 0), 301);
  assert(!session.hid_report_pending());
}

void test_cdc_teardown_and_reconnect_stay_silent_when_neutral() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);

  const uint32_t suppressed_before =
      session.diagnostics().suppressed_hid_duplicates;
  // macOS closing the port on sleep entry: a DTR drop with the pad already
  // neutral must not emit USB input.
  session.on_cdc_disconnected();
  assert(!session.hid_report_pending());
  // Reconnect and a fresh Hello after wake are equally silent.
  session.on_cdc_connected(1000);
  assert(!session.hid_report_pending());
  negotiate(session);
  assert(!session.hid_report_pending());
  assert(session.diagnostics().suppressed_hid_duplicates ==
         suppressed_before + 3);

  // Real input that changes the view still transmits immediately.
  session.on_frame(state_frame(1, 1, 500), 1001);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 1);
}

void test_usb_remount_retransmits_the_safety_neutral() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  assert(!session.hid_report_pending());
  // Identical bytes, but a re-enumerated host has seen nothing: every mount
  // must put the baseline neutral back on the wire so the driver publishes
  // the controller.
  session.on_hid_mounted();
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  assert(session.pending_hid_report().hat == 8);
  session.mark_hid_report_sent();
  session.on_hid_mounted();
  assert(session.hid_report_pending());
  session.mark_hid_report_sent();
}

void test_unsent_changes_coalesce_and_cancel() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session, 0xffff);

  session.on_frame(state_frame(0, 3, 1000), 0);
  session.on_frame(state_frame(1, 5, 2000), 1);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 5);
  // Returning to the delivered view cancels the unsent change outright.
  session.on_frame(state_frame(2, 0, 0), 2);
  assert(!session.hid_report_pending());

  // A change after the cancellation still transmits.
  session.on_frame(state_frame(3, 6, 700), 3);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 6);
}

void test_deferred_state_matching_the_neutral_is_not_resent() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session, 0xffff);
  session.on_frame(state_frame(0, 9, 3000), 0);
  session.mark_hid_report_sent();

  // A sequence gap on a frame that itself carries neutral: the safety
  // neutral transmits (the delivered view is active), and the deferred
  // ordinary neutral then matches the delivered view and must not follow.
  session.on_frame(state_frame(5, 0, 0), 1);
  assert(session.diagnostics().sequence_gaps == 1);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 0);
  session.mark_hid_report_sent();
  assert(!session.hid_report_pending());
}

void test_rejected_sends_do_not_advance_the_queue_cache() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session, 0xffff);

  session.on_frame(state_frame(0, 3, 1000), 0);
  assert(session.hid_report_pending());
  // The sketch only marks after xinput_usb::send accepts the transfer; an
  // immediately rejected send must stay pending rather than being treated as
  // a duplicate.
  session.on_frame(state_frame(1, 3, 1000), 1);
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 3);
  session.mark_hid_report_sent();
  session.on_frame(state_frame(2, 3, 1000), 2);
  assert(!session.hid_report_pending());
}

size_t device_info_count(const CapturingSink& sink) {
  size_t count = 0;
  for (const auto& bytes : sink.writes) {
    if (decode_single(bytes).message_type ==
        static_cast<uint8_t>(MessageType::DeviceInfo)) {
      ++count;
    }
  }
  return count;
}

Frame last_device_info(const CapturingSink& sink) {
  Frame found{};
  bool present = false;
  for (const auto& bytes : sink.writes) {
    const Frame frame = decode_single(bytes);
    if (frame.message_type ==
        static_cast<uint8_t>(MessageType::DeviceInfo)) {
      found = frame;
      present = true;
    }
  }
  assert(present);
  return found;
}

void test_device_info_reported_once_after_negotiation() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);
  // Deferred to the tick so the HelloResponse always leaves first.
  assert(device_info_count(sink) == 0);
  session.tick(0);
  assert(device_info_count(sink) == 1);

  const Frame info = last_device_info(sink);
  assert(info.payload_length == scbridge::kDeviceInfoBasePayloadSize);
  assert(info.payload[0] == scbridge::kDeviceInfoFormat);
  assert(info.payload[1] ==
         static_cast<uint8_t>(scbridge::kFirmwareRevision));
  assert(info.payload[2] ==
         static_cast<uint8_t>(scbridge::kFirmwareRevision >> 8U));
  assert(info.payload[3] == 3);
  assert(info.payload[4] == 0);
  assert(info.payload[5] == 0);
  assert(info.payload[6] == 0);
  assert(info.payload[7] ==
         static_cast<uint8_t>(scbridge::InstallReceiptState::Pending));

  session.tick(1);
  session.tick(50);
  assert(device_info_count(sink) == 1);
}

void test_device_info_retries_when_sink_rejects() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);

  sink.reject_next_write = true;
  session.tick(0);
  assert(device_info_count(sink) == 0);
  session.tick(1);
  assert(device_info_count(sink) == 1);
}

void test_device_info_not_sent_before_negotiation() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  session.tick(5);
  assert(device_info_count(sink) == 0);
}

void test_device_info_resent_after_re_hello() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);
  session.tick(0);
  assert(device_info_count(sink) == 1);

  negotiate(session, 100);
  session.tick(1);
  assert(device_info_count(sink) == 2);
}

void test_device_info_cleared_by_disconnect() {
  CapturingSink sink;
  BridgeSession session(sink);
  session.on_cdc_connected(0);
  session.mark_hid_report_sent();
  negotiate(session);
  // Disconnect before the tick that would have sent it: the report belongs
  // to the dead session and must not leak into the next one.
  session.on_cdc_disconnected();
  session.on_cdc_connected(5);
  session.tick(6);
  assert(device_info_count(sink) == 0);
}

}  // namespace

int main() {
  test_crc_and_neutral_vector();
  test_xinput_report_conversion();
  test_xinput_rumble_parser();
  test_stream_recovery_and_splits();
  test_decoder_validation_and_unknown_messages();
  test_decoder_rejects_header_and_payload_errors_then_recovers();
  test_install_receipt_validation_recovery_and_commit_order();
  test_uf2_transition_neutralizes_before_correlated_ready();
  test_receipt_command_records_and_acknowledges_exact_data();
  test_session_negotiation_sequence_and_watchdog();
  test_rumble_latest_refresh_and_safety_zero();
  test_fault_and_disconnect_neutralize();
  test_active_cdc_disconnect_queues_safety_neutral();
  test_identical_refreshes_suppress_hid_but_feed_watchdog();
  test_cdc_teardown_and_reconnect_stay_silent_when_neutral();
  test_usb_remount_retransmits_the_safety_neutral();
  test_unsent_changes_coalesce_and_cancel();
  test_deferred_state_matching_the_neutral_is_not_resent();
  test_rejected_sends_do_not_advance_the_queue_cache();
  test_device_info_reported_once_after_negotiation();
  test_device_info_retries_when_sink_rejects();
  test_device_info_not_sent_before_negotiation();
  test_device_info_resent_after_re_hello();
  test_device_info_cleared_by_disconnect();
  puts("firmware native tests passed");
  return 0;
}
