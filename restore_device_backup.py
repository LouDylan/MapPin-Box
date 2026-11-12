#!/usr/bin/env python3
"""Restore a MapPin-Box device backup to the matching userdata partition over USB."""

import argparse
import hashlib
import os
from pathlib import Path
import shlex
import struct
import subprocess
import sys
import tempfile

from usb_provision import board_port


ROOT = Path(__file__).resolve().parent
MAGIC = b"MAPBKUP1"
LEGACY_MAGIC = b"I4BKUP01"
USERDATA_OFFSET = 0x820000
USERDATA_SIZE = 0x20000
DIGEST_SIZE = 32


def read_backup(path: Path) -> bytes:
    if path.stat().st_size != len(MAGIC) + USERDATA_SIZE + DIGEST_SIZE:
        raise ValueError("备份大小不正确，请选择MapPin-Box的完整备份")
    file = path.read_bytes()
    payload, digest = file[:-DIGEST_SIZE], file[-DIGEST_SIZE:]
    if not payload.startswith((MAGIC, LEGACY_MAGIC)) or hashlib.sha256(payload).digest() != digest:
        raise ValueError("备份格式或校验值不正确")
    return payload[len(MAGIC):]


def partition_table() -> Path:
    output = ROOT / "firmware" / "build-output"
    table = output / "partition-table.bin"
    if not table.is_file():
        table = output / "wrover-e" / "partition-table.bin"
    if not table.is_file():
        raise ValueError("请先运行“编译开发板固件.command”生成分区表")
    data = table.read_bytes()
    # ESP-IDF's partition entry identifies the exact unencrypted NVS range.
    entry = struct.pack("<HBBII16sI", 0x50AA, 1, 2, USERDATA_OFFSET, USERDATA_SIZE, b"userdata", 0)
    if not any(data[i:i + 32] == entry for i in range(0, len(data), 32)):
        raise ValueError("此固件的用户数据分区与备份格式不匹配")
    return table


def restore(port: str, userdata: bytes, table: Path) -> None:
    base = [sys.executable, "-m", "esptool", "--port", port,
            "--baud", "115200", "--before", "default-reset"]
    subprocess.run(base + ["--after", "no-reset", "verify-flash", "0x8000", str(table)],
                   check=True, timeout=45)
    with tempfile.TemporaryDirectory(prefix="mappin-restore-") as directory:
        raw = Path(directory) / "userdata.bin"
        raw.write_bytes(userdata)
        subprocess.run(base + ["--after", "hard-reset", "write-flash", hex(USERDATA_OFFSET), str(raw)],
                       check=True, timeout=120)


def main() -> None:
    parser = argparse.ArgumentParser(description="通过 Mac USB 恢复盒子的完整设备备份")
    parser.add_argument("backup", type=Path, nargs="?")
    parser.add_argument("--port", help="连接多块板时，明确指定目标串口")
    parser.add_argument("--yes", action="store_true", help="确认覆盖目标盒子的用户数据")
    args = parser.parse_args()
    if args.backup is None:
        path = input("把MapPin-Box的完整备份文件拖到此处，按回车\n").strip()
        parts = [path] if Path(path).is_file() else shlex.split(path)
        if len(parts) != 1:
            raise ValueError("请选择一个完整备份文件")
        args.backup = Path(parts[0])
    data = read_backup(args.backup.expanduser())
    table = partition_table()
    port = args.port or board_port()
    if not args.yes:
        print(f"目标串口 {port}")
        if input("请先恢复目标盒子的真实定位。输入「恢复」以用备份替换其配置\n").strip() != "恢复":
            return
    os.umask(0o077)
    restore(port, data, table)
    print("完整备份已写入并通过校验，盒子正在重启。")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        raise SystemExit(f"恢复失败：{error}") from None
