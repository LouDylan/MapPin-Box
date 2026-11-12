import { icons } from './lucide-icons.mjs';
import { toMapPoint, fromMapPoint } from './map-coordinates.mjs';
import { MapServiceError, mapServiceError } from './map-service.mjs';
import { observeGoogleAuthFailure } from './google-sdk.mjs';

// Each renderer owns its overlays and coordinate boundary. The picker has no SDK branches.
export function createMapView({ provider, api }, element, { onReady, onError, onPoint }) {
  const pin = () => { const n = document.createElement('div'); n.className = 'selected-map-pin'; n.innerHTML = icons['map-pin']; return n; };
  const dot = () => { const n = document.createElement('div'); n.className = 'current-map-dot'; n.setAttribute('role', 'img'); n.setAttribute('aria-label', '盒子当前定位'); return n; };
  const global = ['google', 'osm'].includes(provider);
  const initial = global ? { latitude: 20, longitude: 0 } : { latitude: 31.2304, longitude: 121.4737 };
  const initialZoom = global ? 2 : 11;
  let marker = null, currentMarker = null;
  const loaded = () => onReady();
  const point = (lat, lng) => onPoint({ latitude: lat, longitude: lng });
  if (provider === 'amap') {
    const map = new api.Map(element, { viewMode: '2D', zoom: initialZoom, center: toMapPoint(initial), resizeEnable: true, animateEnable: !matchMedia('(prefers-reduced-motion: reduce)').matches, scrollWheel: false });
    map.on('complete', loaded);
    map.on('click', event => { const p = fromMapPoint(event.lnglat.lng, event.lnglat.lat); point(p.latitude, p.longitude); });
    return { zoomIn: () => map.zoomIn(), zoomOut: () => map.zoomOut(), destroy: () => map.destroy(),
      show(place) { const pos = toMapPoint(place); if (!marker) { marker = new api.Marker({ position: pos, content: pin(), anchor: 'bottom-center', clickable: false }); map.add(marker); } else marker.setPosition(pos); map.setZoomAndCenter(16, pos, true); },
      clear() { if (marker) map.remove(marker); marker = null; },
      current(place, center = false) {
        if (!place) { if (currentMarker) map.remove(currentMarker); currentMarker = null; return; }
        const pos = toMapPoint(place);
        if (!currentMarker) { currentMarker = new api.Marker({ position: pos, content: dot(), anchor: 'center', clickable: false, zIndex: 140 }); map.add(currentMarker); }
        else currentMarker.setPosition(pos);
        if (center) map.setZoomAndCenter(16, pos, true);
      } };
  }
  if (provider === 'osm') {
    const map = new api.Map({ container: element, style: 'https://tiles.openfreemap.org/styles/liberty', center: [initial.longitude, initial.latitude], zoom: initialZoom, maxZoom: 19, attributionControl: { compact: false }, dragRotate: false, pitchWithRotate: false, scrollZoom: false });
    let failed = false;
    map.on('idle', () => { if (!failed) loaded(); });
    map.on('error', () => { failed = true; onError(new MapServiceError('国外底图资源加载失败，请重试。')); });
    map.on('click', event => point(event.lngLat.lat, event.lngLat.lng));
    return { zoomIn: () => map.zoomIn(), zoomOut: () => map.zoomOut(), destroy() { marker?.remove(); currentMarker?.remove(); map.remove(); },
      show(place) { const pos = [place.longitude, place.latitude]; if (!marker) marker = new api.Marker({ element: pin(), anchor: 'bottom' }).setLngLat(pos).addTo(map); else marker.setLngLat(pos); map.jumpTo({ center: pos, zoom: 16 }); },
      clear() { marker?.remove(); marker = null; },
      current(place, center = false) {
        if (!place) { currentMarker?.remove(); currentMarker = null; return; }
        const pos = [place.longitude, place.latitude];
        if (!currentMarker) currentMarker = new api.Marker({ element: dot(), anchor: 'center' }).setLngLat(pos).addTo(map);
        else currentMarker.setLngLat(pos);
        if (center) map.jumpTo({ center: pos, zoom: 16 });
      } };
  }
  if (provider === 'google') {
    let unauthorized = false;
    const map = new api.Map(element, { center: { lat: initial.latitude, lng: initial.longitude }, zoom: initialZoom, disableDefaultUI: true, gestureHandling: 'cooperative', clickableIcons: false });
    const unsubscribe = observeGoogleAuthFailure(error => { unauthorized = true; onError(mapServiceError(error, 'google')); });
    api.event.addListenerOnce(map, 'tilesloaded', () => { if (!unauthorized) loaded(); });
    map.addListener('click', event => { if (event.latLng) point(event.latLng.lat(), event.latLng.lng()); });
    class Pin extends api.OverlayView {
      constructor(isCurrent = false) { super(); this.isCurrent = isCurrent; }
      onAdd() { this.node = this.isCurrent ? dot() : pin(); this.node.classList.add(this.isCurrent ? 'google-current-pin' : 'google-map-pin'); this.getPanes().overlayMouseTarget.append(this.node); }
      draw() { if (!this.node || !this.pos) return; const p = this.getProjection().fromLatLngToDivPixel(this.pos); if (p) { this.node.style.left = p.x + 'px'; this.node.style.top = p.y + 'px'; } }
      onRemove() { this.node?.remove(); }
      move(pos) { this.pos = pos; this.draw(); }
    }
    return { zoomIn: () => map.setZoom(map.getZoom() + 1), zoomOut: () => map.setZoom(map.getZoom() - 1), destroy() { unsubscribe(); marker?.setMap(null); currentMarker?.setMap(null); api.event.clearInstanceListeners(map); element.replaceChildren(); },
      show(place) { const pos = new api.LatLng(place.latitude, place.longitude); if (!marker) { marker = new Pin(); marker.pos = pos; marker.setMap(map); } else marker.move(pos); map.setCenter(pos); map.setZoom(16); },
      clear() { marker?.setMap(null); marker = null; },
      current(place, center = false) {
        if (!place) { currentMarker?.setMap(null); currentMarker = null; return; }
        const pos = new api.LatLng(place.latitude, place.longitude);
        if (!currentMarker) { currentMarker = new Pin(true); currentMarker.pos = pos; currentMarker.setMap(map); }
        else currentMarker.move(pos);
        if (center) { map.setCenter(pos); map.setZoom(16); }
      } };
  }
  const map = new api.Map(element.id, { projection: 'EPSG:900913' });
  map.addEventListener('load', loaded);
  map.centerAndZoom(new api.LngLat(initial.longitude, initial.latitude), initialZoom);
  map.addEventListener('click', event => point(event.lnglat.getLat(), event.lnglat.getLng()));
  // The SDK exposes LocalSearch against the same map instance.
  return { nativeMap: map, zoomIn: () => map.zoomIn(), zoomOut: () => map.zoomOut(), destroy() { map.clearOverLays(); map.remove?.(); element.replaceChildren(); },
    show(place) { const pos = new api.LngLat(place.longitude, place.latitude); if (!marker) { const svg = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(icons['map-pin'].replace('<svg ', '<svg fill="white" color="#275dad" ')); marker = new api.Marker(pos, { icon: new api.Icon({ iconUrl: svg, iconSize: new api.Point(34, 40), iconAnchor: new api.Point(17, 40) }) }); map.addOverLay(marker); } else marker.setLngLat(pos); map.centerAndZoom(pos, 16); },
    clear() { if (marker) map.removeOverLay(marker); marker = null; },
    current(place, center = false) {
      if (!place) { if (currentMarker) map.removeOverLay(currentMarker); currentMarker = null; return; }
      const pos = new api.LngLat(place.longitude, place.latitude);
      if (!currentMarker) {
        const svg = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18"><circle cx="9" cy="9" r="7" fill="#275dad" stroke="white" stroke-width="3"/></svg>');
        currentMarker = new api.Marker(pos, { icon: new api.Icon({ iconUrl: svg, iconSize: new api.Point(18, 18), iconAnchor: new api.Point(9, 9) }) }); map.addOverLay(currentMarker);
      } else currentMarker.setLngLat(pos);
      if (center) map.centerAndZoom(pos, 16);
    } };
}
