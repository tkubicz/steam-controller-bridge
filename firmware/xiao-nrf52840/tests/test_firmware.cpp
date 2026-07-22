#include "bridge_protocol.h"
#include "bridge_session.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

#include <vector>

using scbridge::BridgeSession;
using scbridge::DecodeError;
using scbridge::Frame;
using scbridge::HidGamepadReport;
using scbridge::MessageType;
using scbridge::SessionSink;
using scbridge::StreamDecoder;

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
  bool queue_cdc(const uint8_t* data, size_t length) override {
    writes.emplace_back(data, data + length);
    return true;
  }

  std::vector<std::vector<uint8_t>> writes;
};

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

void test_crc_and_neutral_vector() {
  const uint8_t check[] = {'1', '2', '3', '4', '5', '6', '7', '8', '9'};
  assert(scbridge::crc16_ccitt_false(check, sizeof(check)) == 0x29b1);
  const auto neutral = encode(0, MessageType::Neutral);
  const uint8_t expected[] = {0x53, 0x43, 0x01, 0x04, 0x00,
                              0x00, 0x00, 0x00, 0xe7, 0xfb};
  assert(neutral.size() == sizeof(expected));
  assert(memcmp(neutral.data(), expected, sizeof(expected)) == 0);
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
  stream.insert(stream.end(), reserved_axis.begin(), reserved_axis.end());
  stream.insert(stream.end(), neutral.begin(), neutral.end());

  DecoderEvents events;
  StreamDecoder decoder(collect_frame, collect_error, &events);
  decoder.push(stream.data(), stream.size());
  assert(events.errors.size() == 4);
  assert(events.errors[0] == DecodeError::PayloadTooLarge);
  assert(events.errors[1] == DecodeError::UnsupportedVersion);
  assert(events.errors[2] == DecodeError::InvalidPayloadLength);
  assert(events.errors[3] == DecodeError::ReservedAxisValue);
  assert(events.frames.size() == 1);
  assert(events.frames[0].sequence == 9);
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

  session.on_frame(state_frame(5, 9, 3000), 120);
  assert(session.diagnostics().sequence_gaps == 1);
  assert(session.pending_hid_report().buttons == 0);
  session.mark_hid_report_sent();
  assert(session.hid_report_pending());
  assert(session.pending_hid_report().buttons == 9);

  Frame ping{};
  ping.version = 1;
  ping.message_type = static_cast<uint8_t>(MessageType::Ping);
  ping.sequence = 6;
  ping.payload_length = 4;
  ping.payload[0] = 0x78;
  ping.payload[1] = 0x56;
  ping.payload[2] = 0x34;
  ping.payload[3] = 0x12;
  session.on_frame(ping, 121);
  assert(sink.writes.size() == 2);

  DecoderEvents pong_events;
  StreamDecoder pong_decoder(collect_frame, collect_error, &pong_events);
  pong_decoder.push(sink.writes.back().data(), sink.writes.back().size());
  assert(pong_events.frames.size() == 1);
  assert(pong_events.frames[0].message_type ==
         static_cast<uint8_t>(MessageType::Pong));
  assert(memcmp(pong_events.frames[0].payload, ping.payload, 4) == 0);
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
  session.on_cdc_disconnected();
  assert(!session.cdc_connected());
  assert(session.hid_report_pending());
}

}  // namespace

int main() {
  test_crc_and_neutral_vector();
  test_stream_recovery_and_splits();
  test_decoder_validation_and_unknown_messages();
  test_decoder_rejects_header_and_payload_errors_then_recovers();
  test_session_negotiation_sequence_and_watchdog();
  test_fault_and_disconnect_neutralize();
  puts("firmware native tests passed");
  return 0;
}
