import test from 'node:test';
import assert from 'node:assert/strict';
import {makePlace, parseCoordinates, numberText, samePosition} from '../firmware/src/coordinates.mjs';
test('accepts common Chinese and English coordinate separators', () => {
  for (const text of ['31.2304, 121.4737','31.2304，121.4737','31.2304 121.4737','纬度：31.2304；经度：121.4737','(31.2304,121.4737)','lat=31.2304 lng=121.4737'])
    assert.deepEqual(parseCoordinates(text), {latitude:31.2304,longitude:121.4737});
});
test('does not guess coordinate order or accept extra numbers', () => {
  for (const text of ['121.4737,31.2304','31,121,7','上海31.2304 121.4737','31.2.3,121.4','31.2,Infinity',''])
    assert.throws(() => parseCoordinates(text));
});
test('zero and negative coordinates are valid; blank fields are not zero', () => {
  assert.equal(makePlace('零点', '0', '-180').latitude,0);
  assert.throws(() => makePlace('零点','','0'));
  assert.throws(() => makePlace('零点','0',''));
  assert.throws(() => makePlace('零点','1e309','0'));
});
test('Chinese and emoji name limits use Unicode code points', () => {
  assert.equal(makePlace(' 沪 '.trim(),31,121).name,'沪');
  assert.doesNotThrow(() => makePlace('🏠'.repeat(64),31,121));
  assert.throws(() => makePlace('沪'.repeat(65),31,121));
  assert.throws(() => makePlace('甲\n乙',31,121));
});
test('display precision preserves integer coordinates and identifies the active place', () => {
  assert.equal(numberText(100),'100'); assert.equal(numberText(0),'0'); assert.equal(numberText(-0.000000001),'0');
  assert(samePosition({latitude:31.230400001,longitude:121.4737},{latitude:31.2304,longitude:121.4737}));
});
