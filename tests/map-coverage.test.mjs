import test from 'node:test';
import assert from 'node:assert/strict';
import { needsWorldMap } from '../firmware/src/map-coordinates.mjs';

test('overseas simulation gets an actionable domestic-map coverage hint', () => {
  for (const point of [
    { latitude: 48.8583, longitude: 2.2944 },
    { latitude: 35.6586, longitude: 139.7454 },
    { latitude: 40.6892, longitude: -74.0445 },
  ]) assert.equal(needsWorldMap('amap', point), true);
  for (const point of [
    { latitude: 31.2304, longitude: 121.4737 },
    { latitude: 22.3, longitude: 114.2 },
  ]) assert.equal(needsWorldMap('amap', point), false);
});

test('world-capable sources and absent current positions never show this hint', () => {
  for (const provider of ['osm', 'google', 'tianditu'])
    assert.equal(needsWorldMap(provider, { latitude: 48.8583, longitude: 2.2944 }), false);
  assert.equal(needsWorldMap('amap', null), false);
});
