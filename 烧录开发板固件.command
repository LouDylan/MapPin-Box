#!/bin/zsh
set -eu
dir=${0:A:h}
cd "$dir"
source "$dir/tools/esp-env.sh"
image="$dir/firmware/build-output"
case ${MAPPIN_BOARD:-esp32-s3} in
    esp32-s3) chip=esp32s3; mode=qio ;;
    wrover-e) chip=esp32; mode=dio; image="$image/wrover-e" ;;
    *) print -u2 'MAPPIN_BOARD 请选择 esp32-s3 或 wrover-e'; exit 1 ;;
esac
if [[ ! -r "$image/bootloader.bin" || ! -r "$image/partition-table.bin" || ! -r "$image/mappin-box" ]]; then
    print -u2 '请先运行“编译开发板固件.command”。'
    exit 1
fi
port=${1:-$("$dir/.venv/bin/python" -c 'from usb_provision import board_port; print(board_port())')}
espflash flash --port "$port" --non-interactive --baud 460800 \
    --chip "$chip" --flash-size 16mb --flash-mode "$mode" --flash-freq 40mhz \
    --bootloader "$image/bootloader.bin" --partition-table "$image/partition-table.bin" \
    --target-app-partition factory --skip-update-check "$image/mappin-box"
exec "$dir/.venv/bin/python" "$dir/provision_amap.py" --port "$port"
