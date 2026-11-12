#!/bin/zsh
set -eu
dir=${0:A:h}
cd "$dir"
source "$dir/tools/esp-env.sh"
image="$dir/firmware/build-output"
if [[ ! -r "$image/bootloader.bin" || ! -r "$image/partition-table.bin" || ! -r "$image/waypin-box" ]]; then
    print -u2 '请先运行“编译开发板固件.command”。'
    exit 1
fi
port=${1:-$("$dir/.venv/bin/python" -c 'from provision_board import board_port; print(board_port())')}
exec espflash flash --port "$port" --non-interactive --baud 460800 \
    --chip esp32s3 --flash-size 16mb --flash-mode dio --flash-freq 40mhz \
    --bootloader "$image/bootloader.bin" --partition-table "$image/partition-table.bin" \
    --target-app-partition factory --skip-update-check "$image/waypin-box"
