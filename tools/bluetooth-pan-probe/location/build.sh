#!/usr/bin/env bash
set -euo pipefail

probe_dir="$(cd "$(dirname "$0")" && pwd)"
repo_dir="$(cd "$probe_dir/../../.." && pwd)"
. "$repo_dir/tools/esp-env.sh"

export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
export ESP_IDF_SDKCONFIG_DEFAULTS="$probe_dir/../sdkconfig.defaults;$probe_dir/sdkconfig.defaults"
export ESP_IDF_COMPONENTS="main;mappin_pan;mappin_location;esp_driver_uart;esp_event;esp_netif;esp_wifi;esp_http_server;esp_psram;nvs_flash;bt;pthread;esp_timer;vfs;lwip;espressif__mdns"
# Rust uses its own TLS. Retain the legacy, unused PCNT count required by esp-idf-sys.
export BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:-} -UESP_IDF_COMP_MBEDTLS_ENABLED -include \"$probe_dir/esp-idf-compat.h\""
cd "$probe_dir"
cargo +esp build --release --locked

mkdir -p build-output
espflash save-image --chip esp32 --flash-size 16mb --flash-mode dio --flash-freq 40mhz \
    --partition-table "$repo_dir/firmware/partitions.csv" --target-app-partition factory \
    --skip-update-check "${CARGO_TARGET_DIR:-target}/xtensa-esp32-espidf/release/mappin-bluetooth-location-probe" \
    build-output/app.bin
