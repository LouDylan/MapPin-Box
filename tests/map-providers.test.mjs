import test from 'node:test';
import assert from 'node:assert/strict';
import { placesFromPhoton, placesFromTianditu, placeFromGoogle } from '../firmware/src/places.mjs';
import { mapChange, mapServiceError } from '../firmware/src/map-service.mjs';
import { makePlace, storedPlace, samePosition } from '../firmware/src/coordinates.mjs';

const poi = { name: '车站', lonlat: '121.3161097,31.1961169', address: '上海市闵行区' };
test('OSM and Tianditu adapters keep geographic longitude and latitude without a GCJ shift', () => {
  const [tdt] = placesFromTianditu({ getPois: () => [poi, poi, { ...poi, lonlat: '' }, { ...poi, lonlat: '181,31' }] });
  const [osm] = placesFromPhoton({ features: [{ geometry: { type: 'Point', coordinates: [121.3161097, 31.1961169] }, properties: { name: '车站', city: '上海市' } }] });
  for (const place of [tdt, osm]) {
    assert.equal(place.latitude, 31.1961169);
    assert.equal(place.longitude, 121.3161097);
  }
  assert.equal(placesFromTianditu({ getPois: () => [poi, poi] }).length, 1);
});

test('Google runtime locations use the official API WGS84 values; persistence keeps only a reference and user input', () => {
  const runtime = placeFromGoogle({ id: 'ChIJ_example_123', displayName: 'API 商户名', formattedAddress: 'API 地址', location: { lat: () => 31.2, lng: () => 121.4 } }, '用户输入的地点');
  assert.equal(runtime.latitude, 31.2); assert.equal(runtime.longitude, 121.4);
  const saved = storedPlace(runtime);
  assert.deepEqual(saved, { name: '用户输入的地点', source: 'google', google_place_id: 'ChIJ_example_123', query: '用户输入的地点' });
  assert.equal(samePosition(runtime, saved), true);
  assert.equal(samePosition(saved, { ...saved, google_place_id: 'different' }), false);
  const json = JSON.stringify(saved);
  assert.ok(!/latitude|longitude|API/u.test(json));
  assert.deepEqual(storedPlace(makePlace(saved.name, undefined, undefined, saved)), saved);
  assert.throws(() => makePlace('失效引用', undefined, undefined, { source: 'google' }));
  assert.throws(() => makePlace('缺失坐标', null, null));
});

test('switching providers can reuse saved credentials and validates replacements as one complete unit', () => {
  assert.deepEqual(mapChange('osm'), { provider: 'osm' });
  assert.deepEqual(mapChange('amap', '', '', true), { provider: 'amap' });
  assert.deepEqual(mapChange('google', ' g'.trim().repeat(32), ''), { provider: 'google', key: 'g'.repeat(32) });
  assert.throws(() => mapChange('amap', 'k'.repeat(32), '', true));
  assert.throws(() => mapChange('google', '', '', false));
  assert.throws(() => mapChange('unknown'));
  for (const provider of ['google', 'tianditu', 'osm']) {
    const message = mapServiceError(new Error('https://provider.invalid?key=private-value'), provider).message;
    assert.ok(!message.includes('private-value'));
  }
});

test('regional settings identify the region and reuse credentials without supplying secrets again', () => {
  assert.deepEqual(mapChange('osm', '', '', true, 'global'), { provider: 'osm', region: 'global' });
  assert.deepEqual(mapChange('amap', '', '', true, 'domestic'), { provider: 'amap', region: 'domestic' });
  assert.deepEqual(mapChange('google', 'g'.repeat(39), '', false, 'global'), { provider: 'google', region: 'global', key: 'g'.repeat(39) });
  assert.throws(() => mapChange('google', 'g'.repeat(39), '', false, 'domestic'));
  assert.throws(() => mapChange('osm', '', '', true, 'unknown'));
});

test('Google search and favorite replay resolve the same place ID through the selected provider', async t => {
  const { searchPlaces, resolvePlace } = await import('../firmware/src/places.mjs');
  const oldWindow = globalThis.window, calls = [];
  class GooglePlace {
    constructor({ id }) { this.id = id; }
    static async searchByText(request) {
      calls.push(['search', request.textQuery]);
      return { places: [{ id: 'ChIJ_fake_sample', displayName: 'API 返回名称', formattedAddress: 'API 地址', location: { lat: () => 31.2, lng: () => 121.4 } }] };
    }
    async fetchFields({ fields }) {
      calls.push(['resolve', this.id]);
      assert.ok(fields.includes('location'));
      this.displayName = 'API 更新名称'; this.formattedAddress = 'API 更新地址';
      this.location = { lat: () => 31.21, lng: () => 121.41 };
      return { place: this };
    }
  }
  globalThis.window = { google: { maps: { places: { Place: GooglePlace }, importLibrary: async () => ({}) } } };
  t.after(() => { globalThis.window = oldWindow; });
  t.mock.method(globalThis, 'fetch', async url => {
    assert.equal(url, '/api/maps/config');
    return new Response(JSON.stringify({ provider: 'google', key: 'g'.repeat(39) }));
  });
  const [found] = await searchPlaces('用户查询');
  const saved = storedPlace(found), resolved = await resolvePlace(saved);
  assert.deepEqual(calls, [['search', '用户查询'], ['resolve', 'ChIJ_fake_sample']]);
  assert.equal(resolved.latitude, 31.21); assert.equal(resolved.longitude, 121.41);
  assert.equal(resolved.name, 'API 更新名称');
  assert.equal(saved.latitude, undefined);
  window.gm_authFailure();
  await assert.rejects(searchPlaces('后续查询'), error => error.action === 'configure' && /授权/.test(error.message));
});
