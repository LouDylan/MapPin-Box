import { makePlace, samePosition } from './coordinates.mjs';
import { fromMapPoint } from './map-coordinates.mjs';
import { searchLandmarks } from './landmarks.mjs';
import { abortable, loadMapService, bounded, MapServiceError, mapServiceError } from './map-service.mjs';

const clean = (text, max) => Array.from(typeof text === 'string' ? text.replace(/[\u0000-\u001f\u007f-\u009f]/gu, '').trim() : '').slice(0, max).join('');

export function placesFromAmap(status, result) {
  if (status === 'no_data') return [];
  if (status !== 'complete') throw mapServiceError(result);
  if (!Array.isArray(result?.poiList?.pois))
    throw new MapServiceError('高德未返回具体地点，请补充城市名称或更完整的地址。');
  const places = [];
  for (const poi of result.poiList.pois) {
    const longitude = poi?.location?.lng, latitude = poi?.location?.lat;
    if (typeof longitude !== 'number' || typeof latitude !== 'number'
      || !Number.isFinite(longitude) || !Number.isFinite(latitude)
      || Math.abs(longitude) > 180 || Math.abs(latitude) > 90) continue;
    const name = clean(poi.name, 64);
    const parts = [poi.pname, poi.cityname, poi.adname, poi.address].map(value => clean(value, 96)).filter(Boolean);
    const address = clean([...new Set(parts)].join(' '), 96);
    try {
      const point = fromMapPoint(longitude, latitude);
      const place = makePlace(name, point.latitude, point.longitude, { address, source: 'amap' });
      place.mapPoint = [longitude, latitude];
      if (!places.some(old => samePosition(old, place))) places.push(place);
    } catch { /* Invalid records must never become device commands. */ }
  }
  return places;
}

export function placesFromPhoton(result) {
  const places = [];
  for (const feature of result?.features || []) {
    if (feature?.geometry?.type !== 'Point') continue;
    const [lng, lat] = feature.geometry.coordinates || [], p = feature.properties || {};
    if (typeof lat !== 'number' || typeof lng !== 'number') continue;
    try {
      const address = clean([...new Set([p.country, p.state, p.city, p.district, p.street, p.housenumber].filter(Boolean))].join(' '), 96);
      const place = makePlace(clean(p.name || p.street, 64), lat, lng, { address, source: 'openstreetmap' });
      if (!places.some(old => samePosition(old, place))) places.push(place);
    } catch { /* Ignore unusable provider records. */ }
  }
  return places.slice(0, 8);
}
export function placesFromTianditu(result) {
  const places = [];
  for (const poi of result?.getPois?.() || []) {
    const parts = typeof poi.lonlat === 'string' ? poi.lonlat.split(',') : [];
    if (parts.length !== 2 || parts.some(part => !part.trim())) continue;
    const values = parts.map(Number);
    try {
      const place = makePlace(clean(poi.name, 64), values[1], values[0], { address: clean(poi.address, 96), source: 'tianditu' });
      if (!places.some(old => samePosition(old, place))) places.push(place);
    } catch { /* Ignore unusable provider records. */ }
  }
  return places.slice(0, 8);
}
export function placeFromGoogle(poi, query) {
  const place = makePlace(clean(poi.displayName, 64), poi.location?.lat(), poi.location?.lng(), {
    address: clean(poi.formattedAddress, 96), source: 'google', google_place_id: poi.id, query: clean(query, 64),
  });
  place.expiresAt = Date.now() + 15 * 60 * 1000;
  return place;
}
const cache = new Map();
let photonLast = 0;
function callbackResult(run, convert) {
  return bounded(new Promise((resolve, reject) => {
    try { run((...args) => { try { resolve(convert(...args)); } catch (error) { reject(error); } }); }
    catch (error) { reject(error); }
  }), 12000);
}
async function searchWith(service, query, signal, map, report) {
  const { api, provider } = service;
  if (provider === 'amap') return callbackResult(done => {
    new api.PlaceSearch({ city: '全国', cityLimit: false, pageSize: 8, pageIndex: 1, extensions: 'base' }).search(query, done);
  }, placesFromAmap);
  if (provider === 'osm') {
    const photon = (async () => {
      const remaining = 1100 - (Date.now() - photonLast);
      if (remaining > 0) await abortable(new Promise(resolve => setTimeout(resolve, remaining)), signal);
      if (signal?.aborted) throw signal.reason;
      photonLast = Date.now();
      const url = new URL('https://photon.komoot.io/api/');
      url.search = new URLSearchParams({ q: query, limit: '8' });
      const response = await fetch(url, { signal, referrerPolicy: 'strict-origin-when-cross-origin' });
      if (!response.ok) throw new Error(response.status === 429 ? 'QUOTA' : 'unavailable');
      return placesFromPhoton(await response.json());
    })();
    const [address, landmarks] = await Promise.allSettled([photon, searchLandmarks(query, { signal })]);
    if (signal?.aborted) throw signal.reason;
    if (address.status === 'rejected' && landmarks.status === 'rejected') throw address.reason;
    const places = [...(landmarks.status === 'fulfilled' ? landmarks.value : []), ...(address.status === 'fulfilled' ? address.value : [])];
    if (landmarks.status === 'rejected') report('中文地标服务暂不可用；请核对国家，或使用原文地址。');
    if (address.status === 'rejected') {
      if (!places.length) throw address.reason;
      report('地址搜索暂不可用，当前显示地标结果。');
    }
    return places.filter((place, index) => !places.slice(0, index).some(old => samePosition(old, place))).slice(0, 8);
  }
  if (provider === 'google') {
    const { places } = await api.places.Place.searchByText({ textQuery: query, fields: ['id', 'displayName', 'formattedAddress', 'location'], maxResultCount: 8, language: 'zh-CN' });
    return (places || []).flatMap(poi => { try { return [placeFromGoogle(poi, query)]; } catch { return []; } });
  }
  if (!map?.nativeMap) throw new MapServiceError('请等地图加载完成后重试。');
  return callbackResult(done => {
    const search = new api.LocalSearch(map.nativeMap, { pageCapacity: 8, onSearchComplete: done });
    search.search(query.replace(/["\\]/gu, ' '), 1);
  }, result => {
    if (result?.result?.status?.infocode && Number(result.result.status.infocode) !== 1000) throw new MapServiceError('天地图搜索失败，请检查 Key 和服务权限。', 'configure');
    return placesFromTianditu(result);
  });
}
export async function searchPlaces(query, { signal, map, onNotice } = {}) {
  query = String(query).normalize('NFKC').trim().replace(/\s+/gu, ' ');
  if (!query) throw new Error('请输入想去的地点或地址');
  if (Array.from(query).length > 120) throw new Error('搜索内容过长，请缩短地点名称或地址');
  if (signal?.aborted) throw signal.reason;
  const service = await abortable(loadMapService(), signal), key = service.provider + ':' + query;
  const old = cache.get(key);
  if (old && Date.now() - old.at < 15 * 60 * 1000) return old.places;
  let places, notice = '';
  try { places = await abortable(bounded(searchWith(service, query, signal, map, value => { notice = value; }), 15000), signal); }
  catch (error) { throw mapServiceError(error, service.provider); }
  if (signal?.aborted) throw signal.reason;
  if (notice) onNotice?.(notice);
  if (service.provider !== 'google' && !notice) {
    cache.set(key, { at: Date.now(), places });
    if (cache.size > 12) cache.delete(cache.keys().next().value);
  }
  return places;
}
export async function resolvePlace(place, { signal } = {}) {
  if (place.source !== 'google') return makePlace(place.name, place.latitude, place.longitude, place);
  const service = await abortable(loadMapService(), signal);
  if (service.provider !== 'google') throw new MapServiceError('请在地图设置中切换至 Google 地图，再使用这个收藏。', 'configure');
  try {
    const poi = new service.api.places.Place({ id: place.google_place_id });
    await abortable(bounded(poi.fetchFields({ fields: ['id', 'displayName', 'formattedAddress', 'location'] })), signal);
    return placeFromGoogle(poi, place.query || place.name);
  } catch (error) { throw mapServiceError(error, 'google'); }
}
