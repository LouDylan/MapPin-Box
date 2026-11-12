#!/usr/bin/env python3
"""Keep the iOS location service and its userspace tunnel on a live event loop."""

import argparse
import asyncio
import logging
import signal

from pymobiledevice3.remote.userspace_tunnel import UserspaceRsdTunnel
from pymobiledevice3.services.dvt.instruments.dvt_provider import DvtProvider
from pymobiledevice3.services.dvt.instruments.location_simulation import LocationSimulation

from wifi_transport import wifi_rsd

from phones import resolve_udid


async def run(args):
    args.udid = await resolve_udid(args.udid, args.transport)
    if getattr(args, 'prepare', False) and args.transport == 'usb':
        from pymobiledevice3.lockdown import create_using_usbmux
        from pymobiledevice3.services.mobile_image_mounter import auto_mount
        from pymobiledevice3.exceptions import AlreadyMountedError
        async with await create_using_usbmux(serial=args.udid, connection_type='USB') as phone:
            try: await auto_mount(phone)
            except AlreadyMountedError: pass
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)
    if args.seconds is not None:
        loop.call_later(args.seconds, stop.set)

    tunnel = (
        wifi_rsd(args.udid, args.wifi_host, args.wifi_port)
        if args.transport == "wifi"
        else UserspaceRsdTunnel(serial=args.udid)
    )
    async with (
        tunnel as rsd,
        DvtProvider(rsd) as dvt,
        LocationSimulation(dvt) as location,
    ):
        if args.action == "check":
            print("定位服务已就绪；未发送修改定位指令。", flush=True)
            return
        if args.action == "clear":
            await location.clear()
            print("已发送恢复真实定位指令。", flush=True)
            return

        try:
            await location.set(args.latitude, args.longitude)
            print(f"已发送坐标：纬度 {args.latitude}，经度 {args.longitude}。", flush=True)
            print("连接保持中，请在苹果地图查看当前位置。按 Control+C 停止并恢复真实定位。", flush=True)
            await stop.wait()
        finally:
            await location.clear()
            print("已发送恢复真实定位指令。", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("set", "clear", "check"))
    parser.add_argument("latitude", type=float, nargs="?", default=31.2304)
    parser.add_argument("longitude", type=float, nargs="?", default=121.4737)
    parser.add_argument("--udid")
    parser.add_argument("--prepare", action="store_true", help="通过 USB 操作前检查开发者镜像")
    parser.add_argument("--transport", choices=("usb", "wifi"), default="usb")
    parser.add_argument("--wifi-host", help="手机的局域网 IPv4 地址；省略时自动发现")
    parser.add_argument("--wifi-port", type=int, default=49152)
    parser.add_argument("--seconds", type=float, help="设定自动停止并恢复真实定位的秒数")
    args = parser.parse_args()
    if not -90 <= args.latitude <= 90 or not -180 <= args.longitude <= 180:
        parser.error("经纬度超出范围")
    if args.seconds is not None and args.seconds <= 0:
        parser.error("自动停止秒数必须大于 0")
    if not 1 <= args.wifi_port <= 65535:
        parser.error("端口超出范围")
    if args.wifi_host and args.transport != "wifi":
        parser.error("--wifi-host 只能用于 Wi-Fi 模式")
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    asyncio.run(run(args))


if __name__ == "__main__":
    main()
