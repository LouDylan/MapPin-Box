#!/usr/bin/env python3
"""Import existing local iPhone identities over USB serial, without exporting secrets."""

import argparse
import asyncio
import hashlib
import plistlib
import re
import struct
import time

import serial
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from serial.tools.list_ports import comports
from pymobiledevice3.pair_records import (
    generate_host_id,
    get_usbmux_pairing_record,
    iter_remote_pair_records_by_identifier,
)

from phones import resolve_udid


BOARD_BUSY = "invalid pairing record: 请先恢复真实定位并等待操作完成，再更换手机".encode()
PROVISION_ERROR = re.compile(rb"ENIGMA_PROVISION_ERROR ([^\n]*)\n")
ANSI_COLOR = re.compile(rb"\x1b\[[0-9;]*m")


async def pairing_bundle(udid, profile=None):
    lockdown = await get_usbmux_pairing_record(udid)
    if not lockdown:
        raise RuntimeError("Mac 中没有这台 iPhone 的 USB 信任配对资料。")
    remote = next((r for identifier, _, r in iter_remote_pair_records_by_identifier() if identifier == udid), None)
    if not remote:
        raise RuntimeError("缺少无线配对资料，请先完成 RemotePairing。")
    public = Ed25519PrivateKey.from_private_bytes(remote["private_key"]).public_key().public_bytes_raw()
    if public != remote["public_key"]:
        raise RuntimeError("无线配对密钥不匹配。")
    converted = {
        "public_key": remote["public_key"],
        "private_key": remote["private_key"],
        "identifier": generate_host_id(),
        "alt_irk": remote["peer_alt_irk"],
    }
    lockdown = dict(lockdown)
    lockdown["UDID"] = udid
    if profile:
        lockdown["_I4Device"] = {key: str(profile.get(key) or "")[:64] for key in ("name", "product_type", "product_version")}
    lockdown_bytes = plistlib.dumps(lockdown)
    remote_bytes = plistlib.dumps(converted)
    payload = b"ENIGMA-PAIR/2\0" + struct.pack(">I", len(lockdown_bytes)) + lockdown_bytes + remote_bytes
    if len(payload) > 24 * 1024:
        raise RuntimeError("配对资料超过开发板的 24KB 限制。")
    return payload


def board_port():
    ports = [p.device for p in comports() if p.vid == 0x1A86 and p.device.startswith("/dev/cu.")]
    if len(ports) != 1:
        raise RuntimeError("无法唯一确定开发板，请通过 --port 指定串口。")
    return ports[0]


def send_pairing(connection, payload, digest):
    connection.write(f"ENIGMA-PROVISION/1 {len(payload)} {digest}\n".encode())
    connection.flush()
    time.sleep(0.05)
    for offset in range(0, len(payload), 256):
        connection.write(payload[offset:offset + 256])
        connection.flush()
    deadline = time.monotonic() + 20
    receipt = f"ENIGMA_PROVISION_OK {digest}".encode()
    received = b""
    while time.monotonic() < deadline:
        received = (received + connection.read(1024))[-16384:]
        if receipt in received:
            return None
        error = PROVISION_ERROR.search(received)
        if error:
            return ANSI_COLOR.sub(b"", error[1]).strip()
    raise RuntimeError("开发板没有在期限内确认配对导入。")


def provision(port, payload):
    digest = hashlib.sha256(payload).hexdigest()
    with serial.Serial(port=None, baudrate=115200, timeout=0.25, write_timeout=10) as connection:
        connection.port = port
        connection.dtr = False
        connection.rts = False
        connection.open()
        # Some USB-UART drivers pulse reset when opening the port. Let the
        # board finish its memory test and start the provisioning listener.
        time.sleep(3)
        connection.reset_input_buffer()
        connection.write(b"I4-INFO/1\n")
        connection.flush()
        probe_deadline = time.monotonic() + 3
        info = b""
        while time.monotonic() < probe_deadline:
            info = (info + connection.read(1024))[-4096:]
            if b"I4_INFO/1" in info and b"protocol=2" in info:
                break
        else:
            raise RuntimeError("盒子需要配套 v0.8.0 或更新固件。手机准备已完成，尚未替换盒子配对。")
        error = send_pairing(connection, payload, digest)
        if error == BOARD_BUSY:
            # Opening the port can reboot the board. Keep this port open while
            # startup Wi-Fi or another operation finishes, then retry the import.
            print("盒子正在处理任务，等待空闲后自动重试。", flush=True)
            retry_until = time.monotonic() + 35
            while error == BOARD_BUSY and time.monotonic() < retry_until:
                time.sleep(1.5)
                error = send_pairing(connection, payload, digest)
        if error == BOARD_BUSY:
            raise RuntimeError("盒子仍未空闲。请先恢复旧手机的真实定位，并等盒子操作结束后重试。")
        if error is not None:
            raise RuntimeError("开发板拒绝了配对资料，请检查固件启动日志。")
        print(f"配对资料已通过 USB 导入开发板，SHA-256 校验通过（{len(payload)} 字节）。")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--udid")
    parser.add_argument("--port")
    parser.add_argument("--check", action="store_true", help="仅检查本地资料，不访问开发板串口")
    args = parser.parse_args()
    async def bundle():
        return await pairing_bundle(await resolve_udid(args.udid))
    payload = asyncio.run(bundle())
    if args.check:
        print(f"本地配对资料已就绪（{len(payload)} 字节）；未输出密钥或写入开发板。")
        return
    provision(args.port or board_port(), payload)


if __name__ == "__main__":
    main()
