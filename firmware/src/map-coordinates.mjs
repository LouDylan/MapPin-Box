import { wgs84ToGcj02, approximateGcj02ToWgs84 } from './coord-transform.mjs';

// All stored places and device commands are WGS84. AMap alone uses GCJ-02.
export function fromMapPoint(longitude, latitude) {
  let [lng, lat] = approximateGcj02ToWgs84(longitude, latitude);
  for (let i = 0; i < 6; i += 1) {
    const point = wgs84ToGcj02(lng, lat);
    const dx = point[0] - longitude, dy = point[1] - latitude;
    lng -= dx; lat -= dy;
    if (Math.max(Math.abs(dx), Math.abs(dy)) < 1e-8) break;
  }
  return { longitude: Number(lng.toFixed(7)), latitude: Number(lat.toFixed(7)) };
}

export function toMapPoint(place) {
  return wgs84ToGcj02(place.longitude, place.latitude);
}

// A conservative viewport check, not a country-boundary classifier.
// The configured AMap client does not request its separate overseas capability.
export function needsWorldMap(provider, point) {
  if (provider !== 'amap' || !point) return false;
  return point.longitude < 72 || point.longitude > 138 || point.latitude < 0.8 || point.latitude > 56;
}
