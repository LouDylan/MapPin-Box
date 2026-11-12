import argparse
import contextlib
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch

import phones
import configure_iphone as setup

A = '00000000-0000000000000001'
B = '00000000-0000000000000002'

class Context:
    def __init__(self, value=None): self.value = value
    async def __aenter__(self): return self.value
    async def __aexit__(self, *args): return False

class SelectionTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.selected = Path(self.temp.name)/'selected.json'
        self.file_patch = patch.object(phones, 'SELECTED', self.selected)
        self.file_patch.start()
    def tearDown(self):
        self.file_patch.stop(); self.temp.cleanup()

    async def test_one_new_usb_phone_is_not_overridden_by_old_selection(self):
        phones.save_selected(A)
        with patch.object(phones, 'usb_phones', AsyncMock(return_value=[SimpleNamespace(serial=B)])):
            self.assertEqual(await phones.resolve_udid(), B)
        self.assertEqual(self.selected.stat().st_mode & 0o777, 0o600)

    async def test_multiple_usb_phones_use_only_an_explicitly_saved_match(self):
        with patch.object(phones, 'usb_phones', AsyncMock(return_value=[SimpleNamespace(serial=A),SimpleNamespace(serial=B)])):
            with self.assertRaises(RuntimeError): await phones.resolve_udid()
            phones.save_selected(B)
            self.assertEqual(await phones.resolve_udid(), B)

    async def test_usb_selection_never_falls_back_to_a_network_device(self):
        phones.save_selected(A)
        with patch.object(phones, 'list_devices', AsyncMock(return_value=[SimpleNamespace(serial=A,is_usb=False)])):
            with self.assertRaises(RuntimeError): await phones.resolve_udid()

    async def test_wifi_resolution_uses_the_device_chosen_by_setup(self):
        phones.save_selected(B)
        self.assertEqual(await phones.resolve_udid(transport='wifi'),B)
        self.assertEqual(await phones.resolve_udid(A,transport='wifi'),A)

    def test_corrupt_or_invalid_selection_does_not_target_an_arbitrary_device(self):
        self.selected.write_text('invalid JSON')
        self.assertIsNone(phones.selected_udid())
        with self.assertRaises(ValueError): phones.save_selected('wrong identifier')

class SetupTests(unittest.IsolatedAsyncioTestCase):
    async def test_failed_readiness_never_opens_board_serial_or_changes_selected_phone(self):
        args=argparse.Namespace(check=False,prepare_only=False)
        with patch.object(setup,'choose_device',AsyncMock(return_value=B)), \
             patch.object(setup,'ensure_developer_mode',AsyncMock(return_value=dict(name='Test phone',product_type='iPhone',product_version='27.0.1'))), \
             patch.object(setup,'prepare_identity',AsyncMock(side_effect=RuntimeError('服务尚未就绪'))), \
             patch.object(setup,'provision') as provision, patch.object(setup,'save_selected') as save, \
             contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(RuntimeError): await setup.run(args)
        provision.assert_not_called();save.assert_not_called()

    async def test_cancel_before_import_preserves_board_and_selection(self):
        args=argparse.Namespace(check=False,prepare_only=False)
        with patch.object(setup,'choose_device',AsyncMock(return_value=B)), \
             patch.object(setup,'ensure_developer_mode',AsyncMock(return_value=dict(name='Test phone',product_type='iPhone',product_version='27.0.1'))), \
             patch.object(setup,'prepare_identity',AsyncMock()), patch.object(setup,'pairing_bundle',AsyncMock(return_value=b'fixture')), \
             patch.object(setup,'board_port',return_value='fixture-port'),patch.object(setup,'ask',AsyncMock(return_value='q')), \
             patch.object(setup,'provision') as provision,patch.object(setup,'save_selected') as save, \
             contextlib.redirect_stdout(io.StringIO()):
            await setup.run(args)
        provision.assert_not_called();save.assert_not_called()

    async def test_read_only_check_does_not_pair_or_open_a_serial_port(self):
        with patch.object(setup,'usb_phones',AsyncMock(return_value=[])), patch.object(setup,'board_port',return_value='fixture'), \
             patch.object(setup,'provision') as provision, patch.object(setup,'prepare_identity') as prepare, \
             contextlib.redirect_stdout(io.StringIO()):
            await setup.run(argparse.Namespace(check=True,prepare_only=False))
        provision.assert_not_called();prepare.assert_not_called()

    async def test_prepare_verifies_same_phone_and_opens_location_service_without_setting_gps(self):
        lockdown=SimpleNamespace(set_enable_wifi_connections=AsyncMock())
        remote=SimpleNamespace(connect=AsyncMock(side_effect=setup.RemotePairingCompletedError()),close=AsyncMock())
        gps=SimpleNamespace(set=Mock(side_effect=AssertionError('must not set GPS')),clear=Mock(side_effect=AssertionError('must not clear GPS')))
        with patch.object(setup,'create_using_usbmux',AsyncMock(return_value=Context(lockdown))), \
             patch.object(setup,'auto_mount',AsyncMock(side_effect=setup.AlreadyMountedError())), \
             patch.object(setup,'UserspaceRsdTunnel',return_value=Context(SimpleNamespace(udid=B))) as tunnel, \
             patch.object(setup,'CoreDeviceTunnelService',return_value=remote),patch.object(setup,'DvtProvider',return_value=Context('dvt')), \
             patch.object(setup,'LocationSimulation',return_value=Context(gps)), \
             patch('pymobiledevice3.pair_records.iter_remote_pair_records_by_identifier',return_value=[]):
            await setup.prepare_identity(B)
        lockdown.set_enable_wifi_connections.assert_awaited_once_with(True)
        self.assertEqual(tunnel.call_count,2)
        gps.set.assert_not_called();gps.clear.assert_not_called();remote.close.assert_awaited_once()

    async def test_wrong_rsd_identity_stops_before_creating_remote_pairing(self):
        lockdown=SimpleNamespace(set_enable_wifi_connections=AsyncMock())
        with patch.object(setup,'create_using_usbmux',AsyncMock(return_value=Context(lockdown))),patch.object(setup,'auto_mount',AsyncMock()), \
             patch.object(setup,'UserspaceRsdTunnel',return_value=Context(SimpleNamespace(udid=A))),patch.object(setup,'CoreDeviceTunnelService') as remote:
            with self.assertRaises(RuntimeError): await setup.prepare_identity(B)
        remote.assert_not_called()


class ProvisionProtocolTests(unittest.TestCase):
    def test_an_old_firmware_never_receives_the_new_phone_identity(self):
        import provision_board as bridge
        import usb_provision as transport
        connection=Mock()
        manager=Mock(__enter__=Mock(return_value=connection),__exit__=Mock(return_value=False))
        with patch.object(transport.serial,'Serial',return_value=manager),patch.object(transport.time,'sleep'), \
             patch.object(transport.time,'monotonic',side_effect=[0,5]):
            with self.assertRaisesRegex(RuntimeError,'v0.14.0'): bridge.provision('fixture-port',b'private-fixture')
        self.assertEqual([call.args[0] for call in connection.write.call_args_list],[b'MAPPIN-INFO/1\n'])

    def test_supported_firmware_receives_a_digest_checked_identity_only_after_its_reply(self):
        import provision_board as bridge
        import usb_provision as transport
        import hashlib
        payload=b'private-fixture';digest=hashlib.sha256(payload).hexdigest()
        connection=Mock(read=Mock(side_effect=[b'MAPPIN_INFO/1 version=0.14.0 protocol=1\n',f'MAPPIN_PROVISION_OK pairing {digest} saved\n'.encode()]))
        manager=Mock(__enter__=Mock(return_value=connection),__exit__=Mock(return_value=False))
        with patch.object(transport.serial,'Serial',return_value=manager),patch.object(transport.time,'sleep'), \
             contextlib.redirect_stdout(io.StringIO()): bridge.provision('fixture-port',payload)
        writes=[call.args[0] for call in connection.write.call_args_list]
        self.assertEqual(writes[0],b'MAPPIN-INFO/1\n');self.assertTrue(writes[1].startswith(b'MAPPIN-PROVISION/1 pairing '))
        self.assertEqual(writes[2],payload)

    def test_startup_network_busy_retries_without_reopening_serial(self):
        import hashlib
        import provision_board as bridge
        import usb_provision as transport

        payload = b'private-fixture'
        digest = hashlib.sha256(payload).hexdigest()
        busy = f'MAPPIN_PROVISION_ERROR pairing {digest} busy\r\n'.encode()
        connection = Mock(read=Mock(side_effect=[
            b'MAPPIN_INFO/1 version=0.14.0 protocol=1\n',
            busy[:28],
            busy[28:],
            f'MAPPIN_PROVISION_OK pairing {digest} saved\r\n'.encode(),
        ]))
        manager = Mock(__enter__=Mock(return_value=connection), __exit__=Mock(return_value=False))
        with patch.object(transport.serial, 'Serial', return_value=manager) as serial_port, \
             patch.object(transport.time, 'sleep'), contextlib.redirect_stdout(io.StringIO()):
            bridge.provision('fixture-port', payload)
        serial_port.assert_called_once()
        header = f'MAPPIN-PROVISION/1 pairing {len(payload)} {digest}\n'.encode()
        self.assertEqual([call.args[0] for call in connection.write.call_args_list],
                         [b'MAPPIN-INFO/1\n', header, payload, header, payload])

if __name__=='__main__': unittest.main()
