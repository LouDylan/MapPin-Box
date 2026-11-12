#!/bin/zsh
set -eu
dir=${0:A:h}
cd "$dir"
exec "$dir/.venv/bin/python" "$dir/configure_iphone.py" --prepare-only
