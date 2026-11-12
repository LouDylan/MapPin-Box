import { icons } from './lucide-icons.mjs';
import { makePlace, samePosition } from './coordinates.mjs';
import { searchPlaces } from './places.mjs';
import { loadMapService, mapServiceError, MapServiceError, providers, resetMapService } from './map-service.mjs';
import { needsWorldMap } from './map-coordinates.mjs';
import { createMapView } from './map-view.mjs';
import { currentBoxPosition, positionKey } from './current-location.mjs';

export function createPlacePicker({ onApply, onSave, onConfigureMap, onSwitchProvider }) {
  const $ = id => document.getElementById(id);
  let selected = null, context = {}, map = null, service = null;
  let request = 0, controller = null, searchTimer = null, mapTimer = null, contentTimer = null;
  let mapLoading = false, mapReady = false, mapAction = 'retry', mapGeneration = 0;
  let currentPositionKey = '';
  const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');
  const node = (tag, css, text) => { const n = document.createElement(tag); n.className = css; if (text !== undefined) n.textContent = text; return n; };
  const icon = name => { const n = node('span', ''); n.innerHTML = icons[name]; return n; };
  function mapWarning(error) {
    const outside = map && needsWorldMap(service?.provider, selected || currentBoxPosition(context));
    const message = outside ? new MapServiceError('此位置超出当前高德地图的显示范围。', 'worldMap') : mapServiceError(error, service?.provider);
    mapAction = message.action;
    $('place-map').dataset.state = ({ configure: 'configuration-required', worldMap: 'coverage' })[mapAction] || 'error';
    $('map-warning-text').textContent = message.message;
    $('map-retry').textContent = ({ configure: '地图设置', reload: '刷新重试', openMap: '使用开源地图', worldMap: '使用国外地图' })[mapAction] || '重试';
    $('map-warning').hidden = false; $('map-loading').hidden = true;
    $('search-setup').hidden = mapAction !== 'configure';
  }
  async function initMap() {
    if (mapLoading || map) return;
    mapLoading = true; const generation = ++mapGeneration;
    $('place-map').dataset.state = 'loading'; $('map-loading').hidden = false; $('map-warning').hidden = true;
    try {
      const loaded = await loadMapService();
      if (generation !== mapGeneration) return;
      service = loaded;
      $('search-credit').textContent = providers[service.provider].credit;
      $('search-setup').hidden = true;
      mapTimer = setTimeout(() => { if (generation === mapGeneration) mapWarning(new Error('map timeout')); }, 15000);
      map = createMapView(service, $('place-map'), {
        onReady() {
          if (generation !== mapGeneration) return;
          mapReady = true;
          clearTimeout(mapTimer); $('place-map').dataset.state = 'ready';
          $('map-warning').hidden = true; $('map-loading').hidden = true; render();
        },
        onError(error) { if (generation === mapGeneration) mapWarning(error); },
        onPoint(point) { select(makePlace('地图选点', point.latitude, point.longitude, { source: 'manual' }), { scroll: false }); },
      });
      if (selected) map.show(selected);
      currentPositionKey = ''; updateCurrentPosition();
    } catch (error) { if (generation === mapGeneration) mapWarning(error); }
    finally { if (generation === mapGeneration) { mapLoading = false; render(); } }
  }
  function select(place, { scroll = true } = {}) {
    selected = makePlace(place.name, place.latitude, place.longitude, place);
    if (selected.latitude == null || selected.longitude == null) return clearSelection();
    $('selected-name').textContent = selected.name;
    $('selected-address').textContent = selected.address || '';
    $('selected-address').hidden = !selected.address;
    $('place-detail').hidden = false;
    initMap(); map?.show(selected); render();
    if (selected.source === 'google') expireGoogleContent(place.expiresAt);
    if (scroll && matchMedia('(max-width: 680px)').matches) {
      $('destination-query').blur();
      $('place-detail').scrollIntoView({ behavior: reduceMotion.matches ? 'auto' : 'smooth', block: 'center' });
    }
    for (const button of $('search-results').querySelectorAll('button')) button.setAttribute('aria-pressed', String(button.dataset.position === pointKey(selected)));
  }
  function pointKey(place) { return place.google_place_id || place.latitude + ',' + place.longitude; }
  function clearSelection() { selected = null; map?.clear(); $('place-detail').hidden = true; render(); }
  function expireGoogleContent(expiresAt = Date.now() + 15 * 60 * 1000) {
    clearTimeout(contentTimer);
    contentTimer = setTimeout(() => {
      $('search-results').replaceChildren();
      if (selected?.source === 'google') clearSelection();
      if (service?.provider === 'google' && !$('search-view').hidden) {
        $('result-count').textContent = ''; $('search-message').textContent = '结果已过期，请重新搜索。'; $('search-message').hidden = false;
      }
    }, Math.max(0, expiresAt - Date.now()));
  }
  function render() {
    updateCurrentPosition();
    if (map && needsWorldMap(service?.provider, selected || currentBoxPosition(context))) mapWarning();
    else if (mapAction === 'worldMap') {
      mapAction = 'retry'; $('map-warning').hidden = true;
      $('place-map').dataset.state = mapReady ? 'ready' : 'loading';
    }
    const global = ['google', 'osm'].includes(context.provider);
    $('region-domestic').setAttribute('aria-pressed', String(!global));
    $('region-global').setAttribute('aria-pressed', String(global));
    for (const region of ['domestic', 'global']) $('region-' + region).disabled = !context.online || !!context.switchingMap || !!context.busy;
    $('region-progress').hidden = !context.switchingMap;
    const saved = selected && context.favorites?.some(place => samePosition(place, selected));
    $('save-selection').disabled = !selected || !context.online || !!saved || !!context.saving;
    $('save-selection').classList.toggle('saved', !!saved);
    $('save-selection-label').textContent = saved ? '已收藏' : context.saving ? '保存中' : '收藏';
    $('save-selection').setAttribute('aria-label', saved ? '这个地点已收藏' : '收藏选中地点');
    const applied = selected && context.mode === 'simulating' && samePosition(selected, context.location) && !context.busy;
    $('apply-selection').disabled = !selected || !context.online || !context.paired || !!context.busy || !!context.switchingMap;
    $('apply-selection-label').textContent = context.busy ? '执行中…' : applied ? '重新定位' : '定位到这里';
    $('map-recenter').disabled = !currentBoxPosition(context) || !map; $('map-zoom-in').disabled = !map; $('map-zoom-out').disabled = !map;
    const status = !context.online ? '与盒子的连接已中断' : !context.paired ? '请先完成 iPhone 配对' : '';
    $('selection-status').textContent = status; $('selection-status').hidden = !status;
  }
  function updateCurrentPosition() {
    if (!map) return;
    const current = currentBoxPosition(context), key = positionKey(current);
    if (key === currentPositionKey) return;
    currentPositionKey = key;
    map.current(current, !!current && !selected);
  }
  function cancelSearch() {
    request += 1; controller?.abort(); clearTimeout(searchTimer);
    $('destination-submit').disabled = false; $('destination-submit').textContent = '搜索';
    $('clear-query').hidden = !$('destination-query').value;
    $('search-results').removeAttribute('aria-busy');
    $('search-globally').hidden = true;
  }
  function closeSearch() {
    cancelSearch(); $('destination-query').value = ''; $('clear-query').hidden = true;
    $('search-view').hidden = true; $('library-view').hidden = false;
  }
  async function search() {
    const query = $('destination-query').value.trim();
    if (!query) { focus(); return; }
    cancelSearch(); clearSelection(); const id = request; controller = new AbortController(); const active = controller;
    let timedOut = false;
    searchTimer = setTimeout(() => { timedOut = true; active.abort(); }, 30000);
    $('search-view').hidden = false; $('library-view').hidden = true;
    $('search-results').replaceChildren(); $('search-results').setAttribute('aria-busy', 'true');
    $('result-count').textContent = ''; $('search-message').hidden = false;
    $('search-message').textContent = '正在搜索…'; $('search-message').dataset.error = 'false';
    $('destination-submit').disabled = true; $('destination-submit').textContent = '搜索中';
    try {
      let notice = '';
      const places = await searchPlaces(query, { signal: active.signal, map, onNotice: value => { notice = value; } });
      if (id !== request) return;
      $('result-count').textContent = String(places.length);
      $('search-message').textContent = notice || (places.length ? '' : '未找到地点，试试加上城市或完整地址。'); $('search-message').hidden = !!places.length && !notice;
      $('search-globally').hidden = places.length > 0 || ['google', 'osm'].includes(service?.provider);
      const buttons = places.map(place => {
        const button = node('button', 'result-button'); button.type = 'button'; button.dataset.position = pointKey(place);
        button.setAttribute('aria-pressed', 'false'); button.setAttribute('aria-label', '选择' + place.name + (place.address ? '，' + place.address : ''));
        const copy = node('span', 'place-copy'); copy.append(node('span', 'place-name', place.name));
        if (place.address) copy.append(node('span', 'place-address', place.address));
        button.append(icon('map-pin'), copy); button.onclick = () => select(place); return button;
      });
      $('search-results').replaceChildren(...buttons);
      if (service?.provider === 'google') expireGoogleContent(places[0]?.expiresAt);
    } catch (error) {
      if (id !== request) return;
      $('search-message').hidden = false; $('search-message').dataset.error = 'true';
      $('search-message').textContent = timedOut ? '搜索超时，请重试。' : mapServiceError(error, service?.provider).message;
      if (error.action === 'configure') mapWarning(error);
    } finally {
      if (id === request) { clearTimeout(searchTimer); $('destination-submit').disabled = false; $('destination-submit').textContent = '搜索'; $('search-results').removeAttribute('aria-busy'); }
    }
  }
  function focus() { closeSearch(); $('destination-query').focus(); $('destination-form').scrollIntoView({ behavior: reduceMotion.matches ? 'auto' : 'smooth', block: 'center' }); }
  $('destination-form').onsubmit = event => { event.preventDefault(); search(); };
  function switchRegion(region) { onSwitchProvider(context.regionalProviders?.[region] || (region === 'global' ? 'osm' : 'amap')); }
  $('region-domestic').onclick = () => switchRegion('domestic');
  $('region-global').onclick = () => switchRegion('global');
  $('search-globally').onclick = () => switchRegion('global');
  $('destination-query').addEventListener('keydown', event => { if (event.key === 'Enter' && event.isComposing) event.preventDefault(); });
  $('destination-query').addEventListener('input', () => { cancelSearch(); clearSelection(); $('search-view').hidden = true; $('library-view').hidden = false; });
  $('clear-query').onclick = () => { closeSearch(); clearSelection(); $('destination-query').focus(); }; $('configure-map').onclick = onConfigureMap;
  $('map-zoom-in').onclick = () => map?.zoomIn(); $('map-zoom-out').onclick = () => map?.zoomOut(); $('map-recenter').onclick = () => { const current = currentBoxPosition(context); if (current) map?.current(current, true); };
  $('map-retry').onclick = () => {
    if (mapAction === 'configure') { onConfigureMap(); return; }
    if (mapAction === 'openMap') { onSwitchProvider('osm'); return; }
    if (mapAction === 'worldMap') { switchRegion('global'); return; }
    if (mapAction === 'reload') { location.reload(); return; }
    ++mapGeneration; clearTimeout(mapTimer); map?.destroy(); map = null; mapReady = false; initMap();
  };
  $('apply-selection').onclick = () => { if (selected) onApply(selected); };
  $('save-selection').onclick = () => { if (selected) onSave(selected); };
  initMap();
  return { select, focus, async reloadService() {
    cancelSearch(); clearSelection(); clearTimeout(mapTimer); clearTimeout(contentTimer); $('search-results').replaceChildren(); ++mapGeneration;
    map?.destroy(); map = null; service = null; mapLoading = false; mapReady = false; resetMapService();
    await initMap();
    if ($('destination-query').value.trim()) await search();
  }, update(next) { context = next; render(); }, selected: () => selected };
}
