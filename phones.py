"""Select one iPhone explicitly; never fall through to a different network phone."""
import json
import os
import re
from pathlib import Path

from pymobiledevice3.usbmux import list_devices

SELECTED = Path(__file__).with_name('.private') / 'selected-phone.json'

def selected_udid():
    try:
        value = json.loads(SELECTED.read_text())['udid']
        return value if isinstance(value, str) and re.fullmatch(r'[A-Fa-f0-9-]{16,64}', value) else None
    except (OSError, ValueError, KeyError, TypeError):
        return None

def save_selected(udid):
    if not isinstance(udid, str) or not re.fullmatch(r'[A-Fa-f0-9-]{16,64}', udid):
        raise ValueError('手机标识无效')
    SELECTED.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temp = SELECTED.with_suffix('.tmp')
    with os.fdopen(os.open(temp, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600), 'w') as handle:
        json.dump(dict(udid=udid), handle)
    temp.chmod(0o600)
    temp.replace(SELECTED)

def same_id(a, b):
    return a.replace('-', '').lower() == b.replace('-', '').lower()

async def usb_phones():
    return [device for device in await list_devices() if device.is_usb]

async def resolve_udid(udid=None, transport='usb'):
    if udid:
        return udid
    selected = selected_udid()
    if transport == 'usb':
        devices = await usb_phones()
        if len(devices) == 1:
            return devices[0].serial
        if selected:
            match = next((d for d in devices if same_id(d.serial, selected)), None)
            if match:
                return match.serial
        raise RuntimeError('请用数据线连接一台 iPhone；多台手机请先运行“接入新iPhone.command”选择。')
    if selected:
        return selected
    # An existing single-phone installation can be selected without a hard-coded UDID.
    from pymobiledevice3.pair_records import iter_remote_pair_records_by_identifier
    records = list(iter_remote_pair_records_by_identifier())
    if len(records) == 1:
        return records[0][0]
    raise RuntimeError('请先运行“接入新iPhone.command”选择和配置要使用的手机。')
