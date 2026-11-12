#!/bin/zsh
set -eu
dir=${0:A:h}
exec "$dir/.venv/bin/python" "$dir/restore_device_backup.py" "$@"
