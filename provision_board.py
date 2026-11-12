#!/usr/bin/env python3
"""Import existing local iPhone identities over USB serial, without exporting secrets."""

import argparse
import asyncio
import plistlib

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from pymobiledevice3.pair_records import (
    generate_host_id,
    get_usbmux_pairing_record,
    iter_remote_pair_records_by_identifier,
)

from phones import resolve_udid
import usb_provision
from usb_provision import board_port


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
    record = {"remote": plistlib.dumps(converted), "lockdown": plistlib.dumps(lockdown)}
    if profile:
        record["phone"] = {key: str(profile.get(key) or "")[:64] for key in ("name", "product_type", "product_version")}
    payload = plistlib.dumps(record, fmt=plistlib.FMT_BINARY)
    if len(payload) > usb_provision.LIMITS["pairing"]:
        raise RuntimeError("配对资料超过开发板的 24KB 限制。")
    return payload


def provision(port, payload):
    usb_provision.provision(port, "pairing", payload)
    print("配对资料已通过 USB 导入开发板。")


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
