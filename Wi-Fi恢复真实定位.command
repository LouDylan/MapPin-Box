#!/bin/zsh
set -eu
dir=${0:A:h}
cd "$dir"
options=()
if [[ -n ${WAYPIN_WIFI_HOST:-} ]]; then
  options=(--wifi-host "$WAYPIN_WIFI_HOST")
fi
exec "$dir/.venv/bin/python" "$dir/location.py" clear --transport wifi "${options[@]}"
