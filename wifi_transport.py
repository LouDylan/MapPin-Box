"""Reach one paired iPhone over LAN TCP without using usbmux or Apple's tunnel."""

import asyncio
import ipaddress
from contextlib import AsyncExitStack, asynccontextmanager

from pymobiledevice3.bonjour import browse_remotepairing
from pymobiledevice3.pair_records import iter_remote_pair_records_by_identifier
from pymobiledevice3.remote import tunnel_service
from pymobiledevice3.remote.remote_service_discovery import RemoteServiceDiscoveryService
from pymobiledevice3.remote.siphash import validate_auth_tag
from pymobiledevice3.remote.tunnel_service import RemotePairingTunnelService
from pymobiledevice3.remote.userspace_tunnel import UserspaceDialPlane


async def find_phone(udid):
    records = iter_remote_pair_records_by_identifier()
    record = next((r for identifier, _, r in records if identifier == udid), None)
    if not record or "peer_alt_irk" not in record:
        raise RuntimeError("缺少无线配对资料，请先通过 USB 完成 RemotePairing 配对。")

    for answer in await browse_remotepairing(timeout=5):
        identifier = answer.properties.get("identifier", "")
        auth_tag = answer.properties.get("authTag", "")
        if not validate_auth_tag(record["peer_alt_irk"], identifier, auth_tag):
            continue
        for address in answer.addresses:
            ip = ipaddress.ip_address(address.full_ip.split("%", 1)[0])
            if ip.version == 4 and not ip.is_link_local and not ip.is_loopback:
                return str(ip), answer.port
    raise RuntimeError("没有发现这台 iPhone 的局域网地址；请解锁并连接与 Mac 相同的 Wi-Fi。")


@asynccontextmanager
async def wifi_rsd(udid, host=None, port=49152):
    if host is None:
        host, port = await find_phone(udid)
    provider = RemotePairingTunnelService(udid, host, port)
    previous = tunnel_service.USE_USERSPACE_TUNNEL
    try:
        async with AsyncExitStack() as stack:
            stack.push_async_callback(provider.close)
            await asyncio.wait_for(provider.connect(autopair=False), timeout=15)
            print(f"Wi-Fi 直连：{host}:{port}（RemotePairing / TCP）。", flush=True)
            tunnel_service.USE_USERSPACE_TUNNEL = True
            tunnel = await stack.enter_async_context(provider.start_tcp_tunnel())
            tunnel.client.tun.set_peer(tunnel.address)
            dial = await stack.enter_async_context(UserspaceDialPlane(tunnel.client.tun, tunnel.address))
            rsd = RemoteServiceDiscoveryService(
                (tunnel.address, tunnel.port),
                open_connection=dial.dial,
                auxiliary_metadata=tunnel.auxiliary_metadata,
            )
            stack.push_async_callback(rsd.close)
            await asyncio.wait_for(rsd.connect(), timeout=25)
            if rsd.udid != udid:
                raise RuntimeError("无线连接的设备身份不匹配。")
            print(f"Wi-Fi 开发者服务已连接：{rsd.product_type} / iOS {rsd.product_version}。", flush=True)
            yield rsd
    finally:
        tunnel_service.USE_USERSPACE_TUNNEL = previous
