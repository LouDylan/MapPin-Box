#!/bin/zsh
set -eu
dir=${0:A:h}
cd "$dir"
exec "$dir/.venv/bin/python" "$dir/location.py" set 31.2304 121.4737 --prepare
