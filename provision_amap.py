#!/usr/bin/env python3
"""Save the Mac's private AMap profile, or apply it to a connected MapPin-Box."""

import argparse
from getpass import getpass
import json
import os
from pathlib import Path
import re
import tempfile

from usb_provision import board_port, provision


PROFILE = Path.home() / ".config" / "mappin-box" / "amap.json"
FIELDS = ("key", "security_code")
VALUE = re.compile(r"[A-Za-z0-9_-]{16,128}")


def normalized(value):
    if not isinstance(value, dict) or any(not isinstance(value.get(field), str) for field in FIELDS):
        raise ValueError("需要高德 Web 端（JS API）Key 和安全密钥。")
    profile = {field: value[field].strip() for field in FIELDS}
    if not all(VALUE.fullmatch(item) for item in profile.values()):
        raise ValueError("高德 Key 或安全密钥格式不正确。")
    return profile


def save_profile(value, path=PROFILE):
    profile = normalized(value)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".amap-", delete=False) as output:
        temp = Path(output.name)
        try:
            json.dump(profile, output)
            output.flush()
            os.replace(temp, path)
        finally:
            temp.unlink(missing_ok=True)


def apply_profile(port=None, path=PROFILE):
    try:
        contents = path.read_text()
    except FileNotFoundError:
        return None
    try:
        profile = normalized(json.loads(contents))
    except (ValueError, UnicodeError) as error:
        raise ValueError("本机高德配置无效，请运行「保存高德烧录配置」重新保存。") from error
    payload = json.dumps(profile, separators=(",", ":")).encode()
    return provision(port or board_port(), "amap", payload)


def main():
    parser = argparse.ArgumentParser(description="保存或写入本机高德烧录配置")
    parser.add_argument("--save", action="store_true", help="只在本机保存一次凭证")
    parser.add_argument("--port", help="目标盒子的 USB 串口")
    args = parser.parse_args()
    if args.save:
        save_profile({"key": getpass("高德 Web 端（JS API）Key："), "security_code": getpass("高德安全密钥：")})
        print("本机高德配置已保存，今后烧录新板时会自动写入。")
        return
    result = apply_profile(args.port)
    print({
        None: "本机尚未保存高德配置；可运行「保存高德烧录配置」完成一次配置。",
        "written": "已写入本机高德配置，可直接打开地图和搜索地点。",
        "kept": "板上已配置高德，保留现有凭证。",
    }[result])


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        raise SystemExit(f"高德配置未写入：{error}") from None
