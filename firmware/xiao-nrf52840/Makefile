ARDUINO_CLI ?= arduino-cli
CXX ?= c++
CORE_VERSION := 1.1.13
BOARD_URL := https://files.seeedstudio.com/arduino/package_seeeduino_boards_index.json
FQBN := Seeeduino:nrf52:xiaonRF52840
SKETCH := $(CURDIR)
BUILD_DIR := $(CURDIR)/build
NATIVE_TEST := $(BUILD_DIR)/firmware-tests
ARDUINO_DATA = $(shell $(ARDUINO_CLI) config get directories.data)
UF2CONV = $(ARDUINO_DATA)/packages/Seeeduino/hardware/nrf52/$(CORE_VERSION)/tools/uf2conv/uf2conv.py
PORT ?=

# The core's bundled adafruit-nrfutil is a frozen Click application that aborts
# unless the environment names a UTF-8 locale. Non-interactive shells and CI
# runners routinely leave LANG and LC_ALL unset, so force a locale that both
# macOS and glibc provide.
LC_ALL ?= C.UTF-8
LANG ?= C.UTF-8
export LC_ALL
export LANG

.PHONY: setup test build flash artifacts clean

setup:
	$(ARDUINO_CLI) core update-index --additional-urls $(BOARD_URL)
	$(ARDUINO_CLI) core install Seeeduino:nrf52@$(CORE_VERSION) --additional-urls $(BOARD_URL)

test:
	mkdir -p $(BUILD_DIR)
	$(CXX) -std=c++17 -Wall -Wextra -Werror -pedantic \
		-Isrc src/bridge_protocol.cpp src/bridge_session.cpp \
		tests/test_firmware.cpp -o $(NATIVE_TEST)
	$(NATIVE_TEST)

build:
	mkdir -p $(BUILD_DIR)
	PATH="$(CURDIR)/tools:$(PATH)" $(ARDUINO_CLI) compile --fqbn $(FQBN) \
		--build-property compiler.cpp.extra_flags="-std=gnu++17" \
		--output-dir $(BUILD_DIR) $(SKETCH)

flash: build
	@test -n "$(PORT)" || (echo "PORT is required, e.g. make flash PORT=/dev/cu.usbmodem1101" >&2; exit 2)
	PATH="$(CURDIR)/tools:$(PATH)" $(ARDUINO_CLI) upload --fqbn $(FQBN) \
		--port $(PORT) --input-dir $(BUILD_DIR)

artifacts: build
	mkdir -p $(BUILD_DIR)/artifacts
	@test -f $(UF2CONV) || (echo "UF2 converter missing; run make setup" >&2; exit 1)
	$(CURDIR)/tools/python $(UF2CONV) -c -f 0xADA52840 \
		-o $(BUILD_DIR)/artifacts/steam-controller-bridge-xiao-nrf52840.uf2 \
		$(BUILD_DIR)/xiao-nrf52840.ino.hex
	cp $(BUILD_DIR)/xiao-nrf52840.ino.zip \
		$(BUILD_DIR)/artifacts/steam-controller-bridge-xiao-nrf52840-dfu.zip

clean:
	rm -rf $(BUILD_DIR)
