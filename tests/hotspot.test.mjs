import test from 'node:test';
import assert from 'node:assert/strict';
import { hotspotChange } from '../firmware/src/hotspot.mjs';

test('leaving the hotspot password blank requests preservation, not an open AP', () => {
  assert.deepEqual(hotspotChange('我的盒子', ''), {ssid:'我的盒子', password:null});
});

test('SSID validation measures UTF-8 bytes', () => {
  assert.equal(hotspotChange('家'.repeat(10)).ssid, '家'.repeat(10));
  assert.throws(() => hotspotChange('家'.repeat(11)));
  for (const ssid of ['', '   ', 'bad\nname']) assert.throws(() => hotspotChange(ssid));
});

test('passwords are printable ASCII WPA2 passphrases and are never trimmed', () => {
  assert.equal(hotspotChange('MapPin', ' pass word ').password, ' pass word ');
  for (const value of ['1234567', 'a'.repeat(64), '密码密码密码', 'hello\nworld'])
    assert.throws(() => hotspotChange('MapPin', value));
});
