export class InputError extends Error {
  constructor(message, field) { super(message); this.field = field; }
}
export function makePlace(name, latitude, longitude, details = {}) {
  name = String(name).trim();
  if (!name) throw new InputError('请输入地点名称', 'name');
  if (Array.from(name).length > 64 || /[\u0000-\u001f\u007f-\u009f]/u.test(name))
    throw new InputError('地点名称最多 64 个字，不能包含换行', 'name');
  const reference = details.source === 'google' && latitude == null && longitude == null;
  if (!reference && (latitude == null || String(latitude).trim() === '' || !Number.isFinite(Number(latitude)) || Math.abs(Number(latitude)) > 90))
    throw new InputError('请输入 -90 到 90 之间的纬度', 'latitude');
  if (!reference && (longitude == null || String(longitude).trim() === '' || !Number.isFinite(Number(longitude)) || Math.abs(Number(longitude)) > 180))
    throw new InputError('请输入 -180 到 180 之间的经度', 'longitude');
  const place = reference ? { name } : { name, latitude: Number(latitude), longitude: Number(longitude) };
  const address = String(details.address || '').trim(), source = String(details.source || '').trim();
  if (Array.from(address).length > 96 || /[\u0000-\u001f\u007f-\u009f]/u.test(address))
    throw new InputError('地点地址格式无效', 'name');
  if (source.length > 32 || /[\u0000-\u001f\u007f-\u009f]/u.test(source))
    throw new InputError('地点来源格式无效', 'name');
  if (address) place.address = address;
  if (source) place.source = source;
  if (source === 'google') {
    const id = String(details.google_place_id || ''), query = String(details.query || '').trim();
    if (!/^[a-z\d_-]{1,256}$/iu.test(id) || !query || Array.from(query).length > 64 || /[\u0000-\u001f\u007f-\u009f]/u.test(query))
      throw new InputError('Google 地点引用无效，请重新搜索', 'name');
    place.google_place_id = id; place.query = query;
  }
  return place;
}
export function parseCoordinates(text) {
  const clean = String(text).replace(/(?:纬度|经度|latitude|longitude|lat|lng|lon)\s*[:：=]?/giu, ' ')
    .replace(/[()（）\[\]]/gu, ' ').trim();
  const parts = clean.split(/[\s,，;；/]+/u);
  const decimal = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/u;
  if (parts.length !== 2 || parts.some(part => !decimal.test(part)))
    throw new InputError('请粘贴一组经纬度，纬度在前，例如 31.2304, 121.4737', 'paste');
  const place = makePlace('坐标', parts[0], parts[1]);
  return { latitude: place.latitude, longitude: place.longitude };
}
export function numberText(value) {
  const text = Number(value).toFixed(7).replace(/\.?0+$/u, '');
  return text === '-0' ? '0' : text;
}
export function coordinatesText(place) { if (place.latitude == null || place.longitude == null) return ''; return `${numberText(place.latitude)}, ${numberText(place.longitude)}`; }
export function samePosition(a, b) {
  if (!a || !b) return false;
  if (a.source === 'google' || b.source === 'google') return a.source === b.source && !!a.google_place_id && a.google_place_id === b.google_place_id;
  return !!coordinatesText(a) && coordinatesText(a) === coordinatesText(b);
}

export function storedPlace(place) {
  const value = makePlace(place.name, place.latitude, place.longitude, place);
  if (value.source !== 'google') return value;
  return { name: value.query, source: 'google', google_place_id: value.google_place_id, query: value.query };
}
