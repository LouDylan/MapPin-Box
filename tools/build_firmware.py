#!/usr/bin/env python3
"""Build the selected MapPin-Box firmware and collect its Cargo artifacts."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

FIRMWARE = Path(__file__).resolve().parents[1] / "firmware"
OUTPUT = FIRMWARE / "build-output"
CHIPS = {"esp32-s3": "esp32s3", "wrover-e": "esp32"}
PAN_IDF = "commit:c54fc57d386a4810be28add5e027968c04f910f1"


def build(chip: str) -> tuple[Path, Path]:
    executable = sdk_build = None
    command = ["cargo", "+esp", "build", "--release", "--locked", "--target",
               f"xtensa-{chip}-espidf",
               "--message-format=json-render-diagnostics"]
    env = dict(os.environ, MCU=chip)
    if chip == "esp32":
        env.setdefault("ESP_IDF_VERSION", PAN_IDF)
        # The newer SDK removed the legacy Mbed TLS crypto headers. CoreDevice
        # uses Rust TLS; skip the obsolete bindings while keeping ESP-TLS.
        compat = FIRMWARE / "esp32-pan-compat.h"
        env["BINDGEN_EXTRA_CLANG_ARGS"] = (
            env.get("BINDGEN_EXTRA_CLANG_ARGS", "")
            + f' -UESP_IDF_COMP_MBEDTLS_ENABLED -include "{compat}"'
        )
    with subprocess.Popen(command, cwd=FIRMWARE, env=env, text=True, stdout=subprocess.PIPE) as cargo:
        for line in cargo.stdout:
            message = json.loads(line)
            reason = message["reason"]
            if reason == "compiler-artifact" and message["target"]["name"] == "mappin-box":
                if message["executable"]:
                    executable = Path(message["executable"])
            elif reason == "build-script-executed":
                package = message["package_id"].split("#")[-1]
                if package.startswith("esp-idf-sys@"):
                    sdk_build = Path(message["out_dir"]) / "build"
            elif reason == "compiler-message":
                print(message["message"].get("rendered") or message["message"]["message"],
                      file=sys.stderr, end="")
        if cargo.wait():
            raise SystemExit(cargo.returncode)
    if executable is None or sdk_build is None:
        raise SystemExit("未找到本次构建的MapPin-Box程序或 ESP-IDF 产物")
    return executable, sdk_build


def main() -> None:
    board = os.environ.get("MAPPIN_BOARD", "esp32-s3")
    if board not in CHIPS:
        raise SystemExit("MAPPIN_BOARD 请选择 esp32-s3 或 wrover-e")
    executable, sdk_build = build(CHIPS[board])
    output = OUTPUT if board == "esp32-s3" else OUTPUT / board
    artifacts = (
        executable,
        sdk_build / "bootloader" / "bootloader.bin",
        sdk_build / "partition_table" / "partition-table.bin",
    )
    for artifact in artifacts:
        if not artifact.is_file():
            raise SystemExit(f"缺少构建产物 {artifact.name}")
    output.mkdir(parents=True, exist_ok=True)
    for artifact in artifacts:
        shutil.copyfile(artifact, output / artifact.name)
    print(f"MapPin-Box固件已构建：{output}")


if __name__ == "__main__":
    main()
