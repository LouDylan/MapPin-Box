#!/usr/bin/env python3
"""Build the ESP32-S3 firmware and collect the artifacts from this Cargo build."""

import json
from pathlib import Path
import shutil
import subprocess
import sys

FIRMWARE = Path(__file__).resolve().parents[1] / "firmware"
OUTPUT = FIRMWARE / "build-output"


def build() -> tuple[Path, Path]:
    executable = sdk_build = None
    command = ["cargo", "+esp", "build", "--release", "--locked",
               "--message-format=json-render-diagnostics"]
    with subprocess.Popen(command, cwd=FIRMWARE, text=True, stdout=subprocess.PIPE) as cargo:
        for line in cargo.stdout:
            message = json.loads(line)
            reason = message["reason"]
            if reason == "compiler-artifact" and message["target"]["name"] == "waypin-box":
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
        raise SystemExit("未找到本次构建的移点盒程序或 ESP-IDF 产物")
    return executable, sdk_build


def main() -> None:
    executable, sdk_build = build()
    artifacts = (
        executable,
        sdk_build / "bootloader" / "bootloader.bin",
        sdk_build / "partition_table" / "partition-table.bin",
    )
    for artifact in artifacts:
        if not artifact.is_file():
            raise SystemExit(f"缺少构建产物 {artifact.name}")
    OUTPUT.mkdir(exist_ok=True)
    for artifact in artifacts:
        shutil.copyfile(artifact, OUTPUT / artifact.name)
    print(f"移点盒固件已构建：{OUTPUT}")


if __name__ == "__main__":
    main()
