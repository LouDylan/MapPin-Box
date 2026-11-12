#!/bin/zsh
set -eu
dir=${0:A:h}
exec "$dir/.venv/bin/python" "$dir/provision_amap.py" --save
