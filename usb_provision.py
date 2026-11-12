"""Frame and verify local provisioning requests over the board's USB serial port."""

import hashlib
import re
import time

import serial
from serial.tools.list_ports import comports


INFO = b"MAPPIN-INFO/1\n"
PROTOCOL = re.compile(rb"MAPPIN_INFO/1 version=[^\s]+ protocol=1\r?\n")
LIMITS = {"pairing": 24 * 1024, "amap": 1024}


def board_port():
    ports = [p.device for p in comports() if p.vid == 0x1A86 and p.device.startswith("/dev/cu.")]
    if len(ports) != 1:
        raise RuntimeError("无法唯一确定开发板，请指定串口。")
    return ports[0]


def exchange(connection, kind, payload, digest):
    connection.write(f"MAPPIN-PROVISION/1 {kind} {len(payload)} {digest}\n".encode())
    connection.flush()
    time.sleep(0.05)
    for offset in range(0, len(payload), 256):
        connection.write(payload[offset:offset + 256])
        connection.flush()
    reply = re.compile(rb"MAPPIN_PROVISION_(OK|ERROR) " + f"{kind} {digest}".encode() + rb" ([a-z]+)\r?\n")
    deadline = time.monotonic() + 20
    received = b""
    while time.monotonic() < deadline:
        received = (received + connection.read(1024))[-4096:]
        match = reply.search(received)
        if match:
            return match[1], match[2].decode()
    raise RuntimeError("开发板没有确认写入，请检查 USB 连接。")


def provision(port, kind, payload):
    if kind not in LIMITS or not 1 <= len(payload) <= LIMITS[kind]:
        raise ValueError("写入数据的类型或大小不正确。")
    digest = hashlib.sha256(payload).hexdigest()
    with serial.Serial(port=None, baudrate=115200, timeout=0.25, write_timeout=10) as connection:
        connection.port = port
        connection.dtr = False
        connection.rts = False
        connection.open()
        # Some USB-UART bridges reset the board when opening the port.
        time.sleep(3)
        connection.reset_input_buffer()
        connection.write(INFO)
        connection.flush()
        deadline = time.monotonic() + 4
        received = b""
        while time.monotonic() < deadline:
            received = (received + connection.read(1024))[-4096:]
            if PROTOCOL.search(received):
                break
        else:
            raise RuntimeError("开发板没有返回 MapPin-Box 协议，请确认已烧录 v0.14.0 或更新固件。")

        code, result = exchange(connection, kind, payload, digest)
        if (code, result) == (b"ERROR", "busy"):
            print("盒子正在处理任务，等待空闲后重试。", flush=True)
            deadline = time.monotonic() + 35
            while (code, result) == (b"ERROR", "busy") and time.monotonic() < deadline:
                time.sleep(1.5)
                code, result = exchange(connection, kind, payload, digest)
        if (code, result) == (b"ERROR", "busy"):
            raise RuntimeError("盒子仍未空闲，请先恢复真实定位并等当前操作完成。")
        expected = {"pairing": {"saved"}, "amap": {"written", "kept"}}[kind]
        if code != b"OK" or result not in expected:
            raise RuntimeError("开发板拒绝了配置，请检查固件版本和配置格式。")
        return result
