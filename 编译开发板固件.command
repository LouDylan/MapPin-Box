#!/bin/zsh
set -eu
dir=${0:A:h}
export PATH="$dir/.venv/bin:$PATH"
source "$dir/tools/esp-env.sh"
exec python3 "$dir/tools/build_firmware.py"
