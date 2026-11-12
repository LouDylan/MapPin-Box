import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import provision_amap
import usb_provision


class AmapProfileTests(unittest.TestCase):
    def test_only_the_amap_pair_is_saved_and_sent_to_the_selected_board(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "private" / "amap.json"
            expected = {"key": "k" * 32, "security_code": "s" * 32}
            provision_amap.save_profile(dict(expected, other="discarded"), path)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(path.read_text()), expected)
            payload = json.dumps(expected, separators=(",", ":")).encode()
            digest = hashlib.sha256(payload).hexdigest()
            board = Mock(read=Mock(side_effect=[
                b"MAPPIN_INFO/1 version=0.14.0 protocol=1\n",
                f"MAPPIN_PROVISION_OK amap {digest} kept\r\n".encode(),
            ]))
            manager = Mock(__enter__=Mock(return_value=board), __exit__=Mock(return_value=False))
            with patch.object(usb_provision.serial, "Serial", return_value=manager), \
                 patch.object(usb_provision.time, "sleep"):
                self.assertEqual(provision_amap.apply_profile("board-a", path), "kept")
            self.assertEqual(board.port, "board-a")
            self.assertEqual([c.args[0] for c in board.write.call_args_list], [
                b"MAPPIN-INFO/1\n", f"MAPPIN-PROVISION/1 amap {len(payload)} {digest}\n".encode(), payload
            ])

    def test_absent_or_invalid_profiles_do_not_open_a_board(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(provision_amap, "provision") as serial:
            path = Path(directory) / "amap.json"
            self.assertIsNone(provision_amap.apply_profile("board-a", path))
            path.write_text('{"key":"malformed"}')
            with self.assertRaises(ValueError):
                provision_amap.apply_profile("board-a", path)
            serial.assert_not_called()
