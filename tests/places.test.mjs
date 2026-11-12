import test from 'node:test';
import assert from 'node:assert/strict';
import { placesFromAmap, searchPlaces } from '../firmware/src/places.mjs';
import { fromMapPoint, toMapPoint } from '../firmware/src/map-coordinates.mjs';
import { mapServiceError, validateMapConfig } from '../firmware/src/map-service.mjs';
import { makePlace } from '../firmware/src/coordinates.mjs';

const original = { name: '测试商场', longitude: 121.4737, latitude: 31.2304 };
const [lng, lat] = toMapPoint(original);
const poi = { name: original.name, pname: '上海市', cityname: '上海市', adname: '测试区', address: '测试路1号', location: { lng, lat } };
const response = pois => ({ poiList: { pois } });

test('AMap GCJ-02 locations become WGS84 device commands in the correct order', () => {
  const [place] = placesFromAmap('complete', response([poi]));
  assert.equal(place.latitude, original.latitude);
  assert.equal(place.longitude, original.longitude);
  assert.equal(place.address, '上海市 测试区 测试路1号');
  assert.equal(place.source, 'amap');
  assert.deepEqual(place.mapPoint, [lng, lat]);
  const command = makePlace(place.name, place.latitude, place.longitude, place);
  assert.equal(command.mapPoint, undefined);
  assert.notEqual(command.longitude, lng);
});

test('stored WGS84 places round-trip through the map without accumulating offset', () => {
  for (const place of [original, { longitude: 116.397, latitude: 39.908 }, { longitude: 120.14, latitude: 30.24 }]) {
    const point = fromMapPoint(...toMapPoint(place));
    assert.ok(Math.abs(point.longitude - place.longitude) < 0.0000001);
    assert.ok(Math.abs(point.latitude - place.latitude) < 0.0000001);
  }
  const outside = { longitude: -73.9857, latitude: 40.7484 };
  assert.deepEqual(fromMapPoint(...toMapPoint(outside)), outside);
});

test('invalid provider positions cannot enter saved or device data', () => {
  const pois = [null, {}, { ...poi, location: { lng: '121', lat: 31 } }, { ...poi, location: { lng: 181, lat: 31 } }, { ...poi, location: { lng: 121, lat: NaN } }, { ...poi, name: '' }];
  assert.deepEqual(placesFromAmap('complete', response(pois)), []);
  assert.throws(() => placesFromAmap('complete', {}));
});

test('same-position duplicates collapse while separate branches retain addresses', () => {
  const other = { ...poi, address: '另一条路', location: { lng: lng + 0.1, lat } };
  const places = placesFromAmap('complete', response([poi, poi, other]));
  assert.equal(places.length, 2);
  assert.notEqual(places[0].address, places[1].address);
});

test('no results, authentication failure and quota failure remain distinct', () => {
  assert.deepEqual(placesFromAmap('no_data', { info: 'NO_DATA' }), []);
  assert.throws(() => placesFromAmap('error', { info: 'INVALID_USER_SCODE' }), /凭证未通过验证/u);
  assert.throws(() => placesFromAmap('error', { info: 'CUQPS_HAS_EXCEEDED_THE_LIMIT' }), /上限/u);
  const raw = 'https://example.invalid/?key=private-test-value&scode=private-test-code';
  assert.ok(!mapServiceError(new Error(raw)).message.includes('private-test'));
});

test('first-time credentials require both fields; pasting outer whitespace is allowed', () => {
  assert.deepEqual(validateMapConfig({ key: ' ' + 'k'.repeat(16) + ' ', security_code: 's'.repeat(16) }), { key: 'k'.repeat(16), security_code: 's'.repeat(16) });
  assert.throws(() => validateMapConfig({ key: 'k'.repeat(32) }));
  assert.throws(() => validateMapConfig({ key: 'k'.repeat(32), security_code: 's\nx'.repeat(16) }));
  assert.throws(() => validateMapConfig({ key: 'k'.repeat(129), security_code: 's'.repeat(16) }));
});

test('old favorites remain WGS84 and metadata survives validation', () => {
  assert.deepEqual(makePlace('旧收藏', 31, 121), { name: '旧收藏', latitude: 31, longitude: 121 });
  const [place] = placesFromAmap('complete', response([{ ...poi, name: '甲'.repeat(80), address: '乙'.repeat(150) }]));
  const saved = makePlace(place.name, place.latitude, place.longitude, place);
  assert.equal(Array.from(saved.name).length, 64);
  assert.equal(Array.from(saved.address).length, 96);
});

test('search preserves POI names, caches results and discards cancelled requests', async t => {
  const calls = [];
  const previous = globalThis.window;
  globalThis.window = { AMapLoader: { load: async () => ({ PlaceSearch: class {
    search(query, callback) { calls.push(query); setTimeout(() => callback('complete', response([poi])), 15); }
  } }) } };
  t.after(() => { globalThis.window = previous; });
  t.mock.method(globalThis, 'fetch', async path => {
    assert.equal(path, '/api/maps/config');
    return new Response(JSON.stringify({ key: 'k'.repeat(32), security_code: 's'.repeat(32) }));
  });
  assert.equal((await searchPlaces('江桥万达'))[0].name, original.name);
  await searchPlaces('  江桥万达  ');
  assert.deepEqual(calls, ['江桥万达']);
  const controller = new AbortController();
  const pending = searchPlaces('待取消的查询', { signal: controller.signal });
  await new Promise(resolve => setTimeout(resolve, 1));
  controller.abort();
  await assert.rejects(pending, { name: 'AbortError' });
  await searchPlaces('新查询');
  assert.equal(calls.at(-1), '新查询');
});
