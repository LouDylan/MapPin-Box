#!/usr/bin/env python3
"""Read the board console; closing this monitor does not send a location clear."""

import argparse
import re
import time

import serial

from provision_board import board_port


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port")
    parser.add_argument("--seconds", type=float, default=300)
    parser.add_argument("--reset", action="store_true")
    args = parser.parse_args()
    ansi = re.compile(r"\x1b\[[0-9;]*m")
    with serial.Serial(port=None, baudrate=115200, timeout=0.25) as connection:
        connection.port = args.port or board_port()
        connection.dtr = False
        connection.rts = False
        connection.open()
        if args.reset:
            connection.rts = True
            time.sleep(0.1)
            connection.rts = False
        with open("board-runtime.log", "a", buffering=1) as saved:
            deadline = time.monotonic() + args.seconds
            while time.monotonic() < deadline:
                line = connection.readline().decode("utf-8", errors="replace")
                if line:
                    line = ansi.sub("", line)
                    saved.write(line)
                    print(line, end="", flush=True)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\n串口监视已关闭；未发送恢复定位指令。")
