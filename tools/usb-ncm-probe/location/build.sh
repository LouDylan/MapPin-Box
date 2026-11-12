#!/usr/bin/env bash
set -euo pipefail

probe_dir="$(cd "$(dirname "$0")" && pwd)"
repo_dir="$(cd "$probe_dir/../../.." && pwd)"
export PATH="$repo_dir/.venv/bin:$PATH"
. "$repo_dir/tools/esp-env.sh"
export ESP_IDF_TOOLS_INSTALL_DIR="${ESP_IDF_TOOLS_INSTALL_DIR:-custom:$repo_dir/firmware/.embuild/espressif}"
export ESP_IDF_SDKCONFIG_DEFAULTS="$probe_dir/../sdkconfig.defaults;$probe_dir/sdkconfig.defaults"
# Bind only the ESP-IDF APIs used by Rust. The C component owns the TinyUSB v3 API.
export BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} -UESP_IDF_COMP_ESPRESSIF__ESP_TINYUSB_ENABLED"
cd "$probe_dir"
cargo +esp build --release --locked

mkdir -p build-output
espflash save-image --chip esp32s3 --flash-size 16mb --flash-mode dio --flash-freq 40mhz \
    --partition-table "$repo_dir/firmware/partitions.csv" --target-app-partition factory \
    --skip-update-check "${CARGO_TARGET_DIR:-target}/xtensa-esp32s3-espidf/release/mappin-usb-location-probe" \
    build-output/app.bin
