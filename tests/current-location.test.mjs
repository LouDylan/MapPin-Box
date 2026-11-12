import test from 'node:test';
import assert from 'node:assert/strict';
import { currentBoxPosition, positionKey } from '../firmware/src/current-location.mjs';

const live = { online:true, paired:true, mode:'simulating', location:{latitude:31.2304,longitude:121.4737} };

test('current position comes only from a live successful simulation', () => {
  assert.deepEqual(currentBoxPosition(live), {latitude:31.2304,longitude:121.4737});
  for (const change of [{online:false},{paired:false},{mode:'real'},{mode:'unknown'},{operation:{kind:'set'}},{operation:{kind:'restore'}}])
    assert.equal(currentBoxPosition({...live,...change}), null);
});

test('history, a selected destination and a failed request never become current position', () => {
  assert.equal(currentBoxPosition({online:true,paired:true,mode:'real',last_location:live.location,selected:live.location}), null);
  assert.equal(currentBoxPosition({...live,mode:'unknown',result:{ok:false},last_location:live.location}), null);
});

test('coordinates validate zero, range, numeric type and finite values', () => {
  assert.deepEqual(currentBoxPosition({...live,location:{latitude:0,longitude:0}}),{latitude:0,longitude:0});
  for (const location of [{latitude:'31',longitude:121},{latitude:91,longitude:121},{latitude:31,longitude:181},{latitude:NaN,longitude:0},{latitude:0,longitude:Infinity}])
    assert.equal(currentBoxPosition({...live,location}),null);
  assert.equal(positionKey(null),'');
  assert.equal(positionKey({latitude:0,longitude:0}),'0,0');
});
