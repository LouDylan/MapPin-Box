import test from 'node:test';
import assert from 'node:assert/strict';
import { createMapView } from '../firmware/src/map-view.mjs';

function fixture(t) {
  const old = globalThis.document, maps = [], markers = [], ready = [], errors = [], points = [];
  globalThis.document = { createElement: () => ({ attributes: {}, setAttribute(k, v) { this.attributes[k] = v; } }) };
  t.after(() => { globalThis.document = old; });
  class Map {
    constructor(options) { this.options = options; this.events = {}; this.moves = []; maps.push(this); }
    on(name, fn) { this.events[name] = fn; }
    fire(name, event) { this.events[name]?.(event); }
    jumpTo(value) { this.moves.push(value); }
    zoomIn() {}
    zoomOut() {}
    remove() { this.removed = true; }
  }
  class Marker {
    constructor(options) { this.options = options; markers.push(this); }
    setLngLat(value) { this.point = value; return this; }
    addTo(map) { this.map = map; return this; }
    remove() { this.removed = true; }
  }
  const view = createMapView({ provider: 'osm', api: { Map, Marker } }, {}, {
    onReady: () => ready.push(true), onError: error => errors.push(error), onPoint: point => points.push(point),
  });
  return { view, map: maps[0], markers, ready, errors, points };
}

test('open-map selection, current dot and clicks preserve WGS84 and separate their lifetimes', t => {
  const { view, map, markers, points } = fixture(t);
  const paris = { latitude: 48.8583, longitude: 2.2944 };
  view.show(paris);
  view.current({ latitude: 31.2304, longitude: 121.4737 }, true);
  assert.deepEqual(markers[0].point, [2.2944, 48.8583]);
  assert.deepEqual(markers[1].point, [121.4737, 31.2304]);
  assert.deepEqual(map.moves[1].center, [121.4737, 31.2304]);
  view.clear();
  assert.equal(markers[0].removed, true);
  assert.equal(markers[1].removed, undefined);
  view.current(null);
  assert.equal(markers[1].removed, true);
  map.fire('click', { lngLat: { lat: 48.8583, lng: 2.2944 } });
  assert.deepEqual(points, [paris]);
});

test('a resource failure cannot turn into a false successful map after idle', t => {
  const { map, ready, errors } = fixture(t);
  map.fire('idle');
  assert.equal(ready.length, 1);
  map.fire('error', { error: new Error('https://upstream.invalid/?key=private-value') });
  map.fire('idle');
  assert.equal(ready.length, 1);
  assert.equal(errors.length, 1);
  assert.equal(errors[0].action, 'retry');
  assert.ok(!errors[0].message.includes('private-value'));
});

test('switching sources disposes the map and both overlay types', t => {
  const { view, map, markers } = fixture(t);
  view.show({ latitude: 48.8583, longitude: 2.2944 });
  view.current({ latitude: 48.8583, longitude: 2.2944 });
  view.destroy();
  assert.equal(map.removed, true);
  assert.ok(markers.every(marker => marker.removed));
});
