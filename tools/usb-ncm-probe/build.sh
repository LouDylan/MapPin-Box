#!/usr/bin/env bash
set -euo pipefail

probe_dir="$(cd "$(dirname "$0")" && pwd)"
cd "$probe_dir"
exec idf.py -DIDF_TARGET=esp32s3 "$@"
