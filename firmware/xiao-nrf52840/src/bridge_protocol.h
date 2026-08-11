#pragma once

#include <stddef.h>
#include <stdint.h>

namespace scbridge {

constexpr uint8_t kProtocolVersion = 1;
constexpr size_t kMaxPayloadSize = 256;
constexpr size_t kHeaderSize = 8;
constexpr size_t kChecksumSize = 2;
constexpr size_t kMaxFrameSize = kHeaderSize + kMaxPayloadSize + kChecksumSize;
constexpr size_t kGamepadPayloadSize = 18;
constexpr size_t kRumblePayloadSize = 4;
constexpr size_t kRequestIdPayloadSize = 4;
constexpr size_t kInstallReceiptPayloadSize = 29;

enum class MessageType : uint8_t {
  Hello = 1,
  HelloResponse = 2,
  GamepadState = 3,
  Neutral = 4,
  Ping = 5,
  Pong = 6,
  DeviceInfo = 7,
  Rumble = 8,
  EnterUf2Bootloader = 9,
  Uf2BootloaderReady = 10,
  RecordInstallReceipt = 11,
  InstallReceiptRecorded = 12,
  Error = 255,
};

enum class DecodeError : uint8_t {
  PayloadTooLarge,
  ChecksumMismatch,
  UnsupportedVersion,
  InvalidPayloadLength,
  InvalidHat,
  ReservedAxisValue,
  BufferOverflow,
};

enum class ControlErrorCode : uint16_t {
  Uf2TransitionBusy = 1,
  InstallReceiptRejected = 2,
  InstallReceiptReadbackMismatch = 3,
};

struct Frame {
  uint8_t version;
  uint8_t message_type;
  uint16_t sequence;
  uint16_t payload_length;
  uint8_t payload[kMaxPayloadSize];
};

using FrameCallback = void (*)(void* context, const Frame& frame);
using ErrorCallback = void (*)(void* context, DecodeError error);

uint16_t crc16_ccitt_false(const uint8_t* data, size_t length);

size_t encode_frame(uint16_t sequence, uint8_t message_type,
                    const uint8_t* payload, uint16_t payload_length,
                    uint8_t* output, size_t output_capacity);

class StreamDecoder {
 public:
  StreamDecoder(FrameCallback frame_callback, ErrorCallback error_callback,
                void* context);

  void push(const uint8_t* data, size_t length);
  void reset();

 private:
  void process();
  void discard_prefix(size_t count);
  void emit_error(DecodeError error) const;
  bool validate(const Frame& frame, DecodeError* error) const;

  uint8_t buffer_[kMaxFrameSize];
  size_t buffered_;
  FrameCallback frame_callback_;
  ErrorCallback error_callback_;
  void* context_;
};

}  // namespace scbridge
