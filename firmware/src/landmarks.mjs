import { makePlace } from './coordinates.mjs';

const clean = (text, max) => Array.from(String(text || '').replace(/[\u0000-\u001f\u007f-\u009f]/gu, '').trim()).slice(0, max).join('');
const label = entity => ['zh-hans', 'zh', 'zh-hant', 'en'].map(lang => entity?.labels?.[lang]?.value).find(Boolean) || '';
const nonPlaces = new Set(['Q13406463', 'Q4167410', 'Q4167836', 'Q11424']);
const claim = (entity, property) => (entity?.claims?.[property] || []).find(item => item.rank !== 'deprecated' && item.mainsnak?.snaktype === 'value')?.mainsnak?.datavalue?.value;
export function placesFromWikidata(entities, ids, names = {}) {
  const places = [];
  for (const id of ids) {
    const entity = entities[id], point = claim(entity, 'P625');
    if ((entity?.claims?.P31 || []).some(item => nonPlaces.has(item.mainsnak?.datavalue?.value?.id))) continue;
    if (!point || point.globe !== 'http://www.wikidata.org/entity/Q2' || typeof point.latitude !== 'number' || typeof point.longitude !== 'number') continue;
    const country = names[claim(entity, 'P17')?.id], locality = names[claim(entity, 'P131')?.id];
    const description = ['zh-hans', 'zh', 'en'].map(lang => entity.descriptions?.[lang]?.value).find(Boolean);
    try {
      places.push(makePlace(clean(label(entity), 64), point.latitude, point.longitude,
        { source: 'wikidata', address: clean([...new Set([country, locality, description].filter(Boolean))].join(' '), 96) }));
    } catch { /* Only Earth coordinates validated as device positions may leave this adapter. */ }
  }
  return places;
}
async function request(params, signal) {
  const url = new URL('https://www.wikidata.org/w/api.php');
  url.search = new URLSearchParams({ format: 'json', origin: '*', maxlag: '5', ...params });
  const response = await fetch(url, { signal, credentials: 'omit', referrerPolicy: 'strict-origin-when-cross-origin' });
  if (!response.ok) throw new Error('landmark service unavailable');
  const value = await response.json();
  if (value.error) throw new Error('landmark service unavailable');
  return value;
}
// Manual Han-language landmark lookups only. This is not a worldwide street/merchant geocoder.
export async function searchLandmarks(query, { signal } = {}) {
  if (!/\p{Script=Han}/u.test(query)) return [];
  const controller = new AbortController();
  const abort = () => controller.abort(signal.reason);
  signal?.addEventListener('abort', abort, { once: true });
  const timer = setTimeout(() => controller.abort(), 6500);
  try {
    if (signal?.aborted) throw signal.reason;
    const found = await request({ action: 'wbsearchentities', search: query, language: 'zh', uselang: 'zh', limit: '6' }, controller.signal);
    const ids = (found.search || []).map(item => item.id).filter(id => /^Q\d+$/u.test(id));
    if (!ids.length) return [];
    const result = await request({ action: 'wbgetentities', ids: ids.join('|'), props: 'labels|descriptions|claims', languages: 'zh-hans|zh|en', languagefallback: '1' }, controller.signal);
    const entities = result.entities || {};
    const regions = [...new Set(ids.flatMap(id => [claim(entities[id], 'P17')?.id, claim(entities[id], 'P131')?.id]).filter(id => /^Q\d+$/u.test(id)))];
    let names = {};
    if (regions.length) {
      try {
        const metadata = await request({ action: 'wbgetentities', ids: regions.join('|'), props: 'labels', languages: 'zh-hans|zh|en', languagefallback: '1' }, controller.signal);
        names = Object.fromEntries(Object.entries(metadata.entities || {}).map(([id, entity]) => [id, label(entity)]));
      } catch (error) { if (signal?.aborted) throw error; /* The entity's own description remains available. */ }
    }
    const places = placesFromWikidata(entities, ids, names);
    const normalized = query.normalize('NFKC').trim().toLocaleLowerCase();
    const exact = places.filter(place => place.name.normalize('NFKC').toLocaleLowerCase() === normalized);
    return exact.length ? exact : places;
  } finally { clearTimeout(timer); signal?.removeEventListener('abort', abort); }
}
