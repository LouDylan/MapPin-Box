import test from 'node:test';
import assert from 'node:assert/strict';
import { placesFromWikidata, searchLandmarks } from '../firmware/src/landmarks.mjs';
const claim = value => [{rank:'normal',mainsnak:{snaktype:'value',datavalue:{value}}}];
const earth='http://www.wikidata.org/entity/Q2';
const tower={labels:{zh:{value:'埃菲尔铁塔'}},descriptions:{zh:{value:'法国巴黎的地标建筑'}},claims:{P625:claim({latitude:48.8583,longitude:2.2944,globe:earth}),P17:claim({id:'Q142'}),P131:claim({id:'Q90'})}};
test('Chinese landmark entities retain Earth coordinates, country and locality',()=>{
  const [place]=placesFromWikidata({Q243:tower},['Q243'],{Q142:'法国',Q90:'巴黎'});
  assert.equal(place.name,'埃菲尔铁塔');assert.equal(place.latitude,48.8583);assert.equal(place.longitude,2.2944);
  assert.equal(place.address,'法国 巴黎 法国巴黎的地标建筑');assert.equal(place.source,'wikidata');
});
test('films, missing locations, deprecated positions and non-Earth entities never become GPS points',()=>{
  const other={...tower,claims:{P625:claim({latitude:30,longitude:40,globe:'http://www.wikidata.org/entity/Q405'})}};
  const deprecated={...tower,claims:{P625:[{...claim({latitude:31,longitude:121,globe:earth})[0],rank:'deprecated'}]}};
  assert.deepEqual(placesFromWikidata({Q1:{labels:tower.labels},Q2:other,Q3:deprecated},['Q1','Q2','Q3']),[]);
});
test('Latin street searches do not call the landmark API',async t=>{
  const fetch=t.mock.method(globalThis,'fetch',async()=>{throw Error('must not fetch');});
  assert.deepEqual(await searchLandmarks('5 Avenue Anatole France, Paris'),[]);assert.equal(fetch.mock.callCount(),0);
});
test('label lookup is batched and only the validated point leaves the adapter',async t=>{
  const calls=[];
  t.mock.method(globalThis,'fetch',async url=>{
    const params=new URL(url).searchParams;calls.push(params.get('action'));
    assert.equal(params.get('origin'),'*');
    if(params.get('action')==='wbsearchentities')return new Response(JSON.stringify({search:[{id:'Q243'},{id:'Q162873'}]}));
    if(params.get('props').includes('claims'))return new Response(JSON.stringify({entities:{Q243:tower,Q162873:{...tower,labels:{zh:{value:'埃菲尔铁塔上的文字'}}}}}));
    return new Response(JSON.stringify({entities:{Q142:{labels:{zh:{value:'法国'}}},Q90:{labels:{zh:{value:'巴黎'}}}}}));
  });
  const places=await searchLandmarks('埃菲尔铁塔');
  assert.equal(places.length,1);
  const [place]=places;
  assert.ok(place.address.startsWith('法国 巴黎'));assert.deepEqual(calls,['wbsearchentities','wbgetentities','wbgetentities']);
});

test('a list article with coordinates must not compete with the actual landmark',()=>{
  const article={...tower,claims:{...tower.claims,P31:claim({id:'Q13406463'})}};
  assert.deepEqual(placesFromWikidata({Q162873:article},['Q162873']),[]);
});
