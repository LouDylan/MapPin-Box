#!/bin/zsh
set -eu
dir=${0:A:h}
cd "$dir"
options=()
if [[ -n ${MAPPIN_WIFI_HOST:-} ]]; then
  options=(--wifi-host "$MAPPIN_WIFI_HOST")
fi
exec "$dir/.venv/bin/python" "$dir/location.py" set 31.2304 121.4737 --transport wifi "${options[@]}"
