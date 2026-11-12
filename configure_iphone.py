#!/usr/bin/env python3
"""One-time, local iPhone setup. No location is set and no Apple approval is bypassed."""
import argparse
import asyncio
import logging
import os
import re

from pymobiledevice3.exceptions import AlreadyMountedError, RemotePairingCompletedError
from pymobiledevice3.lockdown import create_using_usbmux
from pymobiledevice3.remote.tunnel_service import CoreDeviceTunnelService
from pymobiledevice3.remote.userspace_tunnel import UserspaceRsdTunnel
from pymobiledevice3.services.dvt.instruments.dvt_provider import DvtProvider
from pymobiledevice3.services.dvt.instruments.location_simulation import LocationSimulation
from pymobiledevice3.services.mobile_image_mounter import auto_mount

from phones import save_selected, usb_phones
from provision_board import board_port, pairing_bundle, provision

async def ask(text):
    return (await asyncio.to_thread(input, text)).strip()

def phone_info(phone):
    values = phone.all_values
    return {key: ''.join(c for c in str(values.get(field) or '') if c.isprintable())[:64] for key, field in
            [('name', 'DeviceName'), ('product_type', 'ProductType'), ('product_version', 'ProductVersion')]}

async def choose_device():
    devices = await usb_phones()
    if not devices:
        raise RuntimeError('未发现 USB iPhone。请用数据线连接手机、解锁，再重新运行。')
    if len(devices) == 1:
        return devices[0].serial
    print('发现多台 USB 设备，请选择要接入的 iPhone：')
    for index, device in enumerate(devices, 1):
        label = '未信任的 USB 设备'
        try:
            async with await create_using_usbmux(serial=device.serial, connection_type='USB', autopair=False) as phone:
                info = phone_info(phone)
                label = f"{info['name']} / {info['product_type']} / iOS {info['product_version']}"
        except Exception:
            pass
        print(f'  {index}. {label}')
    answer = await ask('输入编号；不确定时只保留目标手机的数据线连接：')
    if not answer.isdecimal() or not 1 <= int(answer) <= len(devices):
        raise RuntimeError('没有选择有效手机，未修改配置。')
    return devices[int(answer)-1].serial

async def ensure_developer_mode(udid):
    for _ in range(3):
        async with await create_using_usbmux(serial=udid, connection_type='USB', pair_timeout=90) as phone:
            info = phone_info(phone)
            version = tuple(int(x) for x in re.findall(r'\d+', info['product_version'])[:2])
            if version < (17, 4):
                raise RuntimeError('本助手使用免管理员权限的连接方式，需要 iOS 17.4 或更新版本。')
            if await phone.get_developer_mode_status():
                return info
        print('请在 iPhone 打开“设置 → 隐私与安全性 → 开发者模式”。按手机提示重启、解锁并确认开启。')
        if await ask('完成后保持数据线连接，按回车继续；输入 q 退出：') == 'q':
            raise RuntimeError('已停止。盒子配对尚未更改。')
    raise RuntimeError('开发者模式仍未就绪。请完成手机上的确认，再重新运行。')

async def prepare_identity(udid):
    async with await create_using_usbmux(serial=udid, connection_type='USB', autopair=False) as phone:
        try:
            await auto_mount(phone)
        except AlreadyMountedError:
            pass
        await phone.set_enable_wifi_connections(True)
    # Pair through a fresh USB-backed RSD. This records the modern Wi-Fi identity locally.
    async with UserspaceRsdTunnel(serial=udid) as rsd:
        if rsd.udid.replace('-', '').lower() != udid.replace('-', '').lower():
            raise RuntimeError('准备服务时发现手机身份不一致，已停止。')
        remote = CoreDeviceTunnelService(rsd)
        try:
            await remote.connect(autopair=True)
        except RemotePairingCompletedError:
            # The library saved the record before the peer closed this first pairing connection.
            pass
        finally:
            await remote.close()
    from pymobiledevice3.pair_records import iter_remote_pair_records_by_identifier
    from phones import same_id
    for identifier, path, _ in iter_remote_pair_records_by_identifier():
        if same_id(identifier, udid): path.chmod(0o600)
    # Opening the location service is a readiness check; no set/clear command is sent.
    async with UserspaceRsdTunnel(serial=udid) as rsd, DvtProvider(rsd) as dvt, LocationSimulation(dvt):
        pass

async def run(args):
    if args.check:
        devices = await usb_phones()
        print(f'检测到 {len(devices)} 台 USB 设备。只读检查，未请求信任、准备服务或访问盒子串口。')
        try:
            board_port()
            print('检测到盒子串口，未打开。')
        except RuntimeError:
            print('暂未检测到唯一的盒子串口。')
        return
    print('准备 iPhone 服务\n请用数据线连接并解锁 iPhone。' if args.prepare_only else '接入新 iPhone（需配套 v0.14.0 或更新盒子固件）\n请先让旧手机恢复真实定位，再将新 iPhone 和盒子接到 Mac。')
    udid = await choose_device()
    print('1/4 解锁 iPhone，在手机上确认“信任此电脑”；若出现密码框，请在手机输入。')
    info = await ensure_developer_mode(udid)
    print(f"已选择：{info['name']} / {info['product_type']} / iOS {info['product_version']}")
    print('2/4 准备定位服务与无线配对。首次准备可能需要联网下载文件；如有配对提示，请在手机确认。')
    await prepare_identity(udid)
    payload = await pairing_bundle(udid, profile=info)
    print('3/4 定位服务检查通过，未修改手机位置。')
    if args.prepare_only:
        save_selected(udid)
        print('手机准备完成；盒子尚未更改。需要接入盒子时运行“接入新iPhone.command”。')
        return
    port = board_port()
    answer = await ask('4/4 导入会替换盒子当前手机配对，并可能重启盒子。确认旧手机已恢复真实定位后按回车继续；输入 q 退出：')
    if answer:
        print('尚未写入盒子。')
        return
    await asyncio.to_thread(provision, port, payload)
    save_selected(udid)
    print('接入完成。拔掉 iPhone 数据线，加入盒子的 Wi-Fi 热点，打开 http://192.168.71.1。\n日常可独立供电使用；手机重启或系统更新后若服务不可用，重新运行“准备iPhone定位服务.command”。')

def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='只读检测，不打开盒子串口')
    parser.add_argument('--prepare-only', action='store_true', help='只准备手机，不写入盒子')
    args = parser.parse_args()
    logging.getLogger('pymobiledevice3').setLevel(logging.CRITICAL)
    try:
        asyncio.run(run(args))
    except KeyboardInterrupt:
        print('\n已停止。')
        raise SystemExit(130)
    except Exception as error:
        if isinstance(error, RuntimeError):
            print(str(error))
        else:
            print(f'配置未完成（{type(error).__name__}）。请保持手机解锁、检查信任和互联网连接后重试；尚未成功写入的阶段不会替换盒子配对。')
        raise SystemExit(1) from None

if __name__ == '__main__':
    main()
