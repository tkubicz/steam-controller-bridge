#include "bridge_protocol.h"

#include <string.h>

namespace scbridge {
namespace {

constexpr uint8_t kMagic0 = 0x53;
constexpr uint8_t kMagic1 = 0x43;

uint16_t read_u16(const uint8_t* data) {
  return static_cast<uint16_t>(data[0]) |
         static_cast<uint16_t>(static_cast<uint16_t>(data[1]) << 8U);
}

int16_t read_i16(const uint8_t* data) {
  return static_cast<int16_t>(read_u16(data));
}

void write_u16(uint8_t* output, uint16_t value) {
  output[0] = static_cast<uint8_t>(value & 0xffU);
  output[1] = static_cast<uint8_t>(value >> 8U);
}

bool exact_length(const Frame& frame, size_t expected, DecodeError* error) {
  if (frame.payload_length == expected) {
    return true;
  }
  *error = DecodeError::InvalidPayloadLength;
  return false;
}

}  // namespace

uint16_t crc16_ccitt_false(const uint8_t* data, size_t length) {
  uint16_t crc = 0xffffU;
  for (size_t i = 0; i < length; ++i) {
    crc ^= static_cast<uint16_t>(data[i]) << 8U;
    for (uint8_t bit = 0; bit < 8; ++bit) {
      crc = (crc & 0x8000U) != 0U
                ? static_cast<uint16_t>((crc << 1U) ^ 0x1021U)
                : static_cast<uint16_t>(crc << 1U);
    }
  }
  return crc;
}

size_t encode_frame(uint16_t sequence, uint8_t message_type,
                    const uint8_t* payload, uint16_t payload_length,
                    uint8_t* output, size_t output_capacity) {
  const size_t frame_length = kHeaderSize + payload_length + kChecksumSize;
  if (payload_length > kMaxPayloadSize || output_capacity < frame_length ||
      (payload_length != 0U && payload == nullptr)) {
    return 0;
  }
  output[0] = kMagic0;
  output[1] = kMagic1;
  output[2] = kProtocolVersion;
  output[3] = message_type;
  write_u16(output + 4, payload_length);
  write_u16(output + 6, sequence);
  if (payload_length != 0U) {
    memcpy(output + kHeaderSize, payload, payload_length);
  }
  write_u16(output + kHeaderSize + payload_length,
            crc16_ccitt_false(output, kHeaderSize + payload_length));
  return frame_length;
}

StreamDecoder::StreamDecoder(FrameCallback frame_callback,
                             ErrorCallback error_callback, void* context)
    : buffered_(0),
      frame_callback_(frame_callback),
      error_callback_(error_callback),
      context_(context) {}

void StreamDecoder::reset() { buffered_ = 0; }

void StreamDecoder::push(const uint8_t* data, size_t length) {
  for (size_t i = 0; i < length; ++i) {
    if (buffered_ == sizeof(buffer_)) {
      emit_error(DecodeError::BufferOverflow);
      discard_prefix(1);
    }
    buffer_[buffered_++] = data[i];
    process();
  }
}

void StreamDecoder::process() {
  for (;;) {
    size_t magic_at = buffered_;
    for (size_t i = 0; i + 1 < buffered_; ++i) {
      if (buffer_[i] == kMagic0 && buffer_[i + 1] == kMagic1) {
        magic_at = i;
        break;
      }
    }
    if (magic_at == buffered_) {
      if (buffered_ != 0U && buffer_[buffered_ - 1] == kMagic0) {
        buffer_[0] = kMagic0;
        buffered_ = 1;
      } else {
        buffered_ = 0;
      }
      return;
    }
    if (magic_at != 0U) {
      discard_prefix(magic_at);
    }
    if (buffered_ < kHeaderSize) {
      return;
    }

    const uint16_t payload_length = read_u16(buffer_ + 4);
    if (payload_length > kMaxPayloadSize) {
      emit_error(DecodeError::PayloadTooLarge);
      discard_prefix(1);
      continue;
    }
    const size_t frame_length = kHeaderSize + payload_length + kChecksumSize;
    if (buffered_ < frame_length) {
      return;
    }
    const uint16_t actual_crc = read_u16(buffer_ + frame_length - 2);
    const uint16_t expected_crc =
        crc16_ccitt_false(buffer_, frame_length - kChecksumSize);
    if (actual_crc != expected_crc) {
      emit_error(DecodeError::ChecksumMismatch);
      discard_prefix(1);
      continue;
    }
    if (buffer_[2] != kProtocolVersion) {
      emit_error(DecodeError::UnsupportedVersion);
      discard_prefix(frame_length);
      continue;
    }

    Frame frame{};
    frame.version = buffer_[2];
    frame.message_type = buffer_[3];
    frame.sequence = read_u16(buffer_ + 6);
    frame.payload_length = payload_length;
    if (payload_length != 0U) {
      memcpy(frame.payload, buffer_ + kHeaderSize, payload_length);
    }
    DecodeError error = DecodeError::InvalidPayloadLength;
    if (validate(frame, &error)) {
      if (frame_callback_ != nullptr) {
        frame_callback_(context_, frame);
      }
    } else {
      emit_error(error);
    }
    discard_prefix(frame_length);
  }
}

void StreamDecoder::discard_prefix(size_t count) {
  if (count >= buffered_) {
    buffered_ = 0;
    return;
  }
  memmove(buffer_, buffer_ + count, buffered_ - count);
  buffered_ -= count;
}

void StreamDecoder::emit_error(DecodeError error) const {
  if (error_callback_ != nullptr) {
    error_callback_(context_, error);
  }
}

bool StreamDecoder::validate(const Frame& frame, DecodeError* error) const {
  switch (static_cast<MessageType>(frame.message_type)) {
    case MessageType::Hello:
      return exact_length(frame, 2, error);
    case MessageType::HelloResponse:
      return exact_length(frame, 1, error);
    case MessageType::GamepadState:
      if (!exact_length(frame, kGamepadPayloadSize, error)) {
        return false;
      }
      if (frame.payload[4] > 8U) {
        *error = DecodeError::InvalidHat;
        return false;
      }
      for (size_t offset = 6; offset <= 12; offset += 2) {
        if (read_i16(frame.payload + offset) == INT16_MIN) {
          *error = DecodeError::ReservedAxisValue;
          return false;
        }
      }
      return true;
    case MessageType::Neutral:
      return exact_length(frame, 0, error);
    case MessageType::Ping:
    case MessageType::Pong:
      return exact_length(frame, 4, error);
    case MessageType::DeviceInfo:
      return true;
    case MessageType::Rumble:
      return exact_length(frame, kRumblePayloadSize, error);
    case MessageType::EnterUf2Bootloader:
    case MessageType::Uf2BootloaderReady:
      return exact_length(frame, kRequestIdPayloadSize, error);
    case MessageType::RecordInstallReceipt:
    case MessageType::InstallReceiptRecorded:
      return exact_length(frame, kInstallReceiptPayloadSize, error);
    case MessageType::Error:
      if (frame.payload_length >= 2U) {
        return true;
      }
      *error = DecodeError::InvalidPayloadLength;
      return false;
    default:
      return true;
  }
}

}  // namespace scbridge
