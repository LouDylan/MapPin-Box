import test from 'node:test';
import assert from 'node:assert/strict';
import { mapServiceError } from '../firmware/src/map-service.mjs';
import { createMapView } from '../firmware/src/map-view.mjs';

function page(t) {
  const oldWindow = globalThis.window, oldDocument = globalThis.document;
  const scripts = [];
  globalThis.window = {};
  globalThis.document = {
    createElement: () => ({ remove() { this.removed = true; } }),
    head: { append(node) { scripts.push(node); } },
  };
  t.after(() => { globalThis.window = oldWindow; globalThis.document = oldDocument; });
  return scripts;
}
const sdk = name => import('../firmware/src/google-sdk.mjs?test=' + name);

test('Google waits for the official callback and both libraries; concurrent callers share one script', async t => {
  const scripts = page(t), { loadGoogleSdk } = await sdk('callback');
  const first = loadGoogleSdk('sample-key');
  assert.equal(loadGoogleSdk('sample-key'), first);
  assert.equal(scripts.length, 1);
  const url = new URL(scripts[0].src);
  assert.equal(url.searchParams.get('key'), 'sample-key');
  assert.equal(url.searchParams.get('loading'), 'async');
  let finishPlaces, resolved = false;
  const calls = [], api = { Map: class {}, places: { Place: class {} }, importLibrary(name) {
    calls.push(name);
    return name === 'places' ? new Promise(resolve => { finishPlaces = resolve; }) : Promise.resolve({});
  } };
  window.google = { maps: api };
  first.then(() => { resolved = true; });
  await Promise.resolve();
  assert.equal(resolved, false, 'exported placeholder classes are not a completion signal');
  assert.deepEqual(calls, []);
  window[url.searchParams.get('callback')]();
  assert.deepEqual(calls, ['maps', 'places']);
  await Promise.resolve();
  assert.equal(resolved, false, 'Places must be initialized before searching');
  finishPlaces({});
  assert.equal(await first, api);
});

test('authorization failure before SDK completion rejects immediately and is retained for future calls', async t => {
  const scripts = page(t), { loadGoogleSdk, observeGoogleAuthFailure } = await sdk('early-auth');
  const seen = [], unsubscribe = observeGoogleAuthFailure(error => seen.push(error.code));
  const first = loadGoogleSdk('private-key');
  const rejected = assert.rejects(first, error => error.code === 'GOOGLE_AUTH_FAILED' && !error.message.includes('private-key'));
  window.gm_authFailure();
  await rejected;
  assert.deepEqual(seen, ['GOOGLE_AUTH_FAILED']);
  assert.equal(scripts[0].removed, true);
  await assert.rejects(loadGoogleSdk('private-key'), { code: 'GOOGLE_AUTH_FAILED' });
  assert.equal(scripts.length, 1);
  unsubscribe();
});

test('a failed network load can retry without reusing a failed promise or old callback', async t => {
  const scripts = page(t), { loadGoogleSdk } = await sdk('network');
  const first = loadGoogleSdk('sample-key');
  const rejected = assert.rejects(first, { code: 'GOOGLE_SDK_NETWORK' });
  scripts[0].onerror();
  await rejected;
  assert.equal(scripts[0].removed, true);
  const second = loadGoogleSdk('sample-key');
  assert.equal(scripts.length, 2);
  const callback = new URL(scripts[1].src).searchParams.get('callback');
  assert.notEqual(callback, new URL(scripts[0].src).searchParams.get('callback'));
  window.google = { maps: { importLibrary: async () => ({}) } };
  window[callback]();
  assert.equal(await second, window.google.maps);
});

test('changing a Google key requires a fresh document instead of silently using the old SDK identity', async t => {
  page(t);
  window.google = { maps: { importLibrary: async () => ({}) } };
  const { loadGoogleSdk } = await sdk('new-key');
  await loadGoogleSdk('first-key');
  await assert.rejects(loadGoogleSdk('second-key'), { code: 'GOOGLE_KEY_CHANGED' });
});

test('late authorization failure cannot be hidden by a later tilesloaded event and destroyed maps unsubscribe', async t => {
  page(t);
  let liveReady = 0, liveErrors = 0, deadErrors = 0;
  const maps = [];
  class FakeMap {
    constructor() { maps.push(this); }
    addListener() {}
  }
  const api = { Map: FakeMap, OverlayView: class {}, importLibrary: async () => ({}),
    event: { addListenerOnce(map, name, listener) { map[name] = listener; }, clearInstanceListeners() {} } };
  window.google = { maps: api };
  const { loadGoogleSdk } = await import('../firmware/src/google-sdk.mjs');
  await loadGoogleSdk('sample-key');
  const element = () => ({ replaceChildren() {} });
  const dead = createMapView({ provider: 'google', api }, element(), { onReady() {}, onError() { deadErrors++; }, onPoint() {} });
  dead.destroy();
  const live = createMapView({ provider: 'google', api }, element(), { onReady() { liveReady++; }, onError(error) {
    liveErrors++; assert.equal(error.action, 'configure'); assert.match(error.message, /授权/);
  }, onPoint() {} });
  window.gm_authFailure();
  maps[1].tilesloaded();
  assert.equal(deadErrors, 0);
  assert.equal(liveErrors, 1);
  assert.equal(liveReady, 0);
  live.destroy();
});

test('Google configuration failures identify the required fix without exposing raw credentials', () => {
  const cases = [
    ['BillingNotEnabledMapError', /结算/], ['ApiNotActivatedMapError', /尚未启用/],
    ['ApiTargetBlockedMapError', /API 限制/], ['RefererNotAllowedMapError', /网页地址/],
    ['GOOGLE_AUTH_FAILED', /授权失败/], ['PERMISSION_DENIED', /授权失败/],
  ];
  for (const [code, message] of cases) {
    const error = mapServiceError(new Error(code + ' key=private-value'), 'google');
    assert.equal(error.action, 'configure'); assert.match(error.message, message);
    assert.ok(!error.message.includes('private-value'));
    assert.ok(!error.message.includes('无法连接'));
  }
  assert.equal(mapServiceError({ code: 'GOOGLE_KEY_CHANGED' }, 'google').action, 'reload');
  assert.equal(mapServiceError({ code: 'GOOGLE_SDK_NETWORK' }, 'google').action, 'openMap');
  assert.equal(mapServiceError({ code: 'GOOGLE_SDK_TIMEOUT' }, 'google').action, 'openMap');
});
