# ESP32-S3 QIO bootloader

`esp32s3-qio-80m.bin` is ESP-IDF's second-stage bootloader built for QIO flash at 80 MHz, the
one Arduino-ESP32 ships for this chip. It sets the flash's Quad Enable bit and reads it with 4
data lines, so code, fonts and images take half the memory-bus time they do in DIO, time the
display's scan-out needs (see `espflash.toml`).

- Source: `esp32s3/bin/bootloader_qio_80m.elf` from
  [espressif/esp32-arduino-libs](https://github.com/espressif/esp32-arduino-libs), branch
  `idf-release/v5.1` (ESP-IDF 5.1), sha256
  `0f6b63e58cd9717f294de12b8850f45fbdaad0159a0decf56d44a629e512281b`.
- Converted as Arduino does, with a DIO header (the ROM reads the header's mode before Quad
  Enable is set):

  ```sh
  esptool.py --chip esp32s3 elf2image --flash_mode dio --flash_freq 80m --flash_size 16MB \
      -o esp32s3-qio-80m.bin bootloader_qio_80m.elf
  ```

  sha256 `0654a691d790651a2d4c1a63ee55d13555e987d5cfe9e7a8bb19f593c40faa74` (esptool 4.8.1).

At boot the app logs `flash: read mode QIO`. If the board does not boot with it, remove the
`[idf]` section of `espflash.toml` and flash again: espflash's bundled DIO bootloader is used.
