import { api } from './api.mjs';
import { icons } from './lucide-icons.mjs';
import { makePlace, parseCoordinates, storedPlace } from './coordinates.mjs';
import { mapChange, providers, providerNotes } from './map-service.mjs';
import { hotspotChange } from './hotspot.mjs';

const $ = id => document.getElementById(id);
const page = document.body.dataset.adminPage;
const text = (id, value) => { $(id).textContent = value ?? '—'; };
const node = (tag, content, css = '') => { const value = document.createElement(tag); value.className = css; if (content !== undefined) value.textContent = content; return value; };
const bytes = value => Number.isFinite(value) ? (value >= 1048576 ? (value / 1048576).toFixed(1) + ' MB' : (value / 1024).toFixed(1) + ' KB') : '—';
const uptime = value => value >= 3600 ? `${Math.floor(value / 3600)} 小时 ${Math.floor(value % 3600 / 60)} 分钟` : value >= 60 ? `${Math.floor(value / 60)} 分钟 ${value % 60} 秒` : `${value} 秒`;
let data = null, online = false, loading = true, posting = false, timer = null, inflight = null, toastTimer = null;
let lastWifiResult = null, lastLocationResult = null, ssidInitialized = false, mapsInitialized = false;
let backupPending = false, deviceBackup = null;
let networksSignature = '', eventsSignature = '', confirmedHotspot = null;
const mapRegions = ['domestic', 'global'];
const mapElement = (region, name) => $(`map-${region}-${name}`);
const hotspotStorageSlot = 'waypin.hotspot.pending.v1';
let pendingHotspot = null;
try { pendingHotspot = JSON.parse(sessionStorage.getItem(hotspotStorageSlot)); } catch {}
if (typeof pendingHotspot?.ssid !== 'string' || !Number.isInteger(pendingHotspot?.jobId)) pendingHotspot = null;

function rememberHotspot(value) {
  pendingHotspot = value;
  try { if (value) sessionStorage.setItem(hotspotStorageSlot, JSON.stringify(value)); else sessionStorage.removeItem(hotspotStorageSlot); } catch {}
}
function toast(value, error = false) {
  clearTimeout(toastTimer); text('admin-toast', value);
  $('admin-toast').dataset.error = String(error); $('admin-toast').hidden = false;
  toastTimer = setTimeout(() => { $('admin-toast').hidden = true; }, error ? 7000 : 4000);
}
function message(id, value, error = false) {
  text(id, value); $(id).hidden = false;
  $(id).dataset.error = String(error); $(id).dataset.success = String(!error);
}
function schedule() {
  clearTimeout(timer);
  if (!document.hidden) timer = setTimeout(() => refresh(), online && (data?.wifi.operation || data?.app.operation) ? 1000 : online ? 5000 : 3000);
}
async function refresh(force = false) {
  if (inflight) { await inflight; return force ? refresh() : undefined; }
  clearTimeout(timer);
  inflight = (async () => {
    try {
      const next = await api('/api/admin/status');
      if (!data || data.app.boot_id !== next.app.boot_id) {
        lastWifiResult = next.wifi.result?.id ?? null;
        lastLocationResult = next.app.result?.id ?? null;
      }
      if (next.wifi.result && next.wifi.result.id !== lastWifiResult) {
        lastWifiResult = next.wifi.result.id;
        toast(next.wifi.result.message, !next.wifi.result.ok);
      }
      if (next.app.result && next.app.result.id !== lastLocationResult) {
        lastLocationResult = next.app.result.id; toast(next.app.result.message, !next.app.result.ok);
      }
      data = next; online = true;
    } catch (error) { online = false; text('admin-offline', error.message); }
    loading = false; render();
  })();
  try { await inflight; } finally { inflight = null; schedule(); }
}
function render() {
  text('admin-connection-label', loading ? '连接中' : online ? '在线' : '离线');
  $('admin-connection').dataset.online = String(online); $('admin-offline').hidden = online || loading;
  if (!online && pendingHotspot) text('admin-offline', `请在手机 Wi-Fi 设置中重新加入“${pendingHotspot.ssid}”，页面会自动检测连接。`);
  $('admin-preview-label').hidden = !data?.app.preview; $('admin-preview-notice').hidden = !data?.app.preview;
  $('export-diagnostics').disabled = !data;
  if (!data) {
    for (const control of document.querySelectorAll('main button, main select, #admin-restore')) control.disabled = true;
    if (page === 'network' && pendingHotspot) renderHotspotReconnect();
    return;
  }
  for (const control of document.querySelectorAll('main button, main select')) control.disabled = posting;
  const { app, wifi } = data;
  const networkBusy = !!wifi.operation || app.network_busy;
  const restoring = !!app.restore_queued || app.operation?.kind === 'restore';
  $('admin-restore').disabled = !online || posting || !app.paired || networkBusy || restoring;
  text('admin-restore-label', app.restore_queued ? '恢复已排队' : restoring ? '正在恢复…' : '恢复真实定位');
  ({ system: renderSystem, network: renderNetwork, maps: renderMaps,
    help: renderHelp, location: renderLocation }[page])?.();
}
function renderSystem() {
  const { device: d, app } = data;
  text('dev-version', '固件 v' + app.version);
  text('dev-chip', `${d.chip} / ${d.cores} 核`); text('dev-cpu', d.configured_cpu_mhz + ' MHz');
  text('dev-uptime', uptime(app.uptime)); text('dev-reset', d.reset_reason); text('dev-sdk', d.idf_version);
  text('dev-flash', bytes(d.flash_bytes)); text('dev-internal', bytes(d.internal_free));
  text('dev-minheap', bytes(d.internal_min_free)); text('dev-largest', bytes(d.internal_largest_block));
  text('dev-psram', bytes(d.psram_free) + ' / ' + bytes(d.psram_bytes));
}
function networkLocked() {
  return !online || posting || !!data.app.operation || data.app.mode === 'simulating' || !!data.wifi.operation || data.app.network_busy;
}
function renderNetwork() {
  const { app, wifi } = data;
  text('wifi-ap-name', wifi.ssid); text('wifi-ap-address', 'http://' + wifi.address);
  text('wifi-channel', wifi.channel || '—'); text('wifi-client-count', wifi.client_count == null ? '读取失败' : wifi.client_count + ' 台');
  text('wifi-upstream-name', wifi.upstream.connected ? wifi.upstream.saved_ssid : wifi.upstream.saved_ssid ? wifi.upstream.saved_ssid + '（未连接）' : '未连接');
  text('wifi-upstream-address', wifi.upstream.connected ? (wifi.upstream.address || '—') + ' / ' + (wifi.upstream.dns || '—') : '—');
  $('wifi-clients').replaceChildren(...wifi.clients.map(client => node('li', `设备尾号 ${client.id}，${client.rssi} dBm`)));
  if (!ssidInitialized) {
    $('wifi-ssid').value = wifi.upstream.saved_ssid || ''; $('hotspot-ssid').value = wifi.ssid;
    ssidInitialized = true;
  }
  const locked = networkLocked(), locationBusy = !!app.operation || app.mode === 'simulating';
  for (const id of ['scan-wifi', 'connect-wifi', 'save-hotspot', 'confirm-hotspot']) $(id).disabled = locked;
  $('disconnect-wifi').disabled = locked || !wifi.upstream.saved_ssid;
  $('confirm-wifi-disconnect').disabled = locked || !wifi.upstream.saved_ssid;
  $('wifi-lock').hidden = !locationBusy && !wifi.operation && !app.network_busy;
  text('wifi-lock', locationBusy ? '请先恢复真实定位，再更改 Wi-Fi。' : '正在调整 Wi-Fi，请稍候。');
  if (wifi.operation && wifi.operation.kind !== 'hotspot') message('wifi-message', ({ scan: '正在扫描附近网络', connect: '正在连接上游 Wi-Fi', disconnect: '正在断开上游 Wi-Fi' }[wifi.operation.kind] || '正在调整 Wi-Fi') + `，已等待 ${Math.floor(wifi.operation.elapsed_ms / 1000)} 秒`);
  else if (wifi.result && wifi.result.kind !== 'hotspot') message('wifi-message', wifi.result.message, !wifi.result.ok);
  else text('wifi-message', wifi.upstream.connected ? '盒子已连接上游 Wi-Fi。' : '尚未连接上游网络。');
  renderNetworks(locked); renderHotspotReconnect();
}
function renderHotspotReconnect() {
  const pending = pendingHotspot, wifi = data?.wifi;
  const sameBoot = pending && data?.app.boot_id === pending.bootId;
  const result = sameBoot && wifi?.result?.id === pending?.jobId ? wifi.result : null;
  if (pending && online && result && !result.ok) {
    rememberHotspot(null); message('hotspot-message', result.message, true);
  } else if (pending && online && wifi.ssid === pending.ssid && !wifi.operation && (result?.ok || !sameBoot)) {
    rememberHotspot(null); message('hotspot-message', '热点设置已保存，当前连接正常。');
    $('hotspot-ssid').value = wifi.ssid;
  }
  $('hotspot-reconnect').hidden = !pendingHotspot;
  if (!pendingHotspot) return;
  text('hotspot-target', pendingHotspot.ssid);
  text('hotspot-reconnect-title', online ? '正在应用热点设置，请重新连接' : '热点已断开，请重新连接');
  $('check-hotspot').disabled = posting;
}
function renderNetworks(locked) {
  const signature = JSON.stringify([data.wifi.networks, locked]);
  if (signature === networksSignature) return;
  networksSignature = signature;
  $('wifi-networks').replaceChildren(...data.wifi.networks.map(network => {
    const row = node('div', undefined, 'network-row'), copy = node('div');
    copy.append(node('strong', network.ssid), node('small', `${network.secured ? '需要密码' : '开放网络'}，${network.rssi} dBm，信道 ${network.channel}`));
    const button = node('button', '选择', 'secondary'); button.type = 'button'; button.disabled = locked;
    button.setAttribute('aria-label', '选择网络' + network.ssid);
    button.onclick = () => { $('wifi-ssid').value = network.ssid; $('wifi-password').value = ''; $('wifi-password').focus(); };
    row.append(copy, button); return row;
  }));
}
function renderMaps() {
  if (!mapsInitialized) {
    for (const region of mapRegions) mapElement(region, 'provider').value = data.app.regional_providers?.[region] || (region === 'domestic' ? 'amap' : 'osm');
    mapsInitialized = true;
  }
  for (const region of mapRegions) {
    renderMapFields(region);
    mapElement(region, 'save').disabled = !online || posting;
  }
  text('backup-count', `${data.app.favorites.length} / ${data.app.limits.favorites} 个地点`);
  $('export-favorites').disabled = !data.app.favorites.length;
  $('import-favorites').disabled = !online || posting;
}
function renderDeviceBackup() {
  $('export-device-backup').disabled = backupPending || (!deviceBackup && (!online || posting));
  text('device-backup-label', deviceBackup ? '保存备份文件' : '导出完整备份');
}
function renderHelp() {
  const phone = data.app.phone;
  text('help-phone', phone?.name || (data.app.paired ? '已有配对资料' : '尚未接入'));
  text('help-model', phone ? [phone.product_type, phone.product_version && 'iOS ' + phone.product_version].filter(Boolean).join(' / ') : '');
  $('help-model').hidden = !phone;
  text('help-ssid', data.wifi.ssid);
  $('help-replace').hidden = !data.app.paired;
  renderDeviceBackup();
}
function renderLocation() {
  const { app, debug, wifi } = data;
  const mode = app.operation ? '执行中' : ({ real: '真实定位', simulating: '模拟定位', unknown: '状态待确认' }[app.mode] || '状态待确认');
  text('configured-phone', app.phone?.name || (app.paired ? '已有配对资料' : '尚未接入'));
  text('location-mode', mode); text('pairing-status', app.paired ? '已配置' : '未配置');
  text('phone-address', debug.phone_ip || '尚未记录');
  text('last-location', app.last_location?.name || '暂无记录'); text('last-button', app.button?.message || '暂无操作');
  if (app.operation) message('location-progress', (app.operation.kind === 'restore' ? '正在恢复真实定位' : '正在定位到“' + app.operation.label + '”') + `，已等待 ${Math.floor(app.operation.elapsed_ms / 1000)} 秒`);
  else if (app.result) message('location-progress', app.result.message, !app.result.ok);
  else text('location-progress', '等待操作。');
  const disabled = !online || posting || !app.paired || !!app.operation || !!wifi.operation || app.network_busy;
  $('replay-location').disabled = disabled || !app.last_location;
  $('manual-apply').disabled = disabled; $('manual-save').disabled = !online || posting;
  renderEvents();
}
function renderEvents() {
  const events = data.debug.events, signature = JSON.stringify(events);
  $('clear-events').disabled = !online || posting || !events.length;
  if (signature === eventsSignature) return;
  eventsSignature = signature;
  const rows = [...events].reverse().map(event => {
    const row = node('li'); row.dataset.level = event.level;
    row.append(node('time', `${Math.floor(event.uptime / 60)}:${String(event.uptime % 60).padStart(2, '0')}`), node('span', event.message)); return row;
  });
  $('event-list').replaceChildren(...(rows.length ? rows : [node('li', '暂无记录。', 'empty-event')]));
}
async function post(path, value = {}, target = null) {
  if (posting) return false;
  posting = true; render();
  try {
    const result = await api(path, value);
    if (target) message(target, result.message || '操作已提交。');
    await refresh(true); return true;
  } catch (error) {
    if (target) message(target, error.message, true);
    toast(error.message, true); return false;
  } finally { posting = false; render(); }
}
function download(value, name) {
  const blob = value instanceof Blob ? value : new Blob([JSON.stringify(value, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a'); link.href = url; link.download = name;
  document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url), 30000);
}
const dateName = () => new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Shanghai' });

function setupNetwork() {
  $('scan-wifi').onclick = () => post('/api/wifi/scan');
  $('wifi-form').onsubmit = async event => {
    event.preventDefault();
    const value = { ssid: $('wifi-ssid').value, password: $('wifi-password').value };
    if (!value.ssid || new TextEncoder().encode(value.ssid).length > 32 || /[\u0000-\u001f\u007f-\u009f]/u.test(value.ssid)) { message('wifi-message', 'Wi-Fi 名称需要1至32字节，不能包含控制字符。', true); return; }
    const size = new TextEncoder().encode(value.password).length;
    if ((size && (size < 8 || size > 63)) || /[\u0000-\u001f\u007f-\u009f]/u.test(value.password)) { message('wifi-message', 'Wi-Fi 密码需要8至63字节，开放网络请留空。', true); return; }
    if (await post('/api/wifi/connect', value, 'wifi-message')) $('wifi-password').value = '';
  };
  $('disconnect-wifi').onclick = () => { $('wifi-disconnect-confirm').hidden = false; $('confirm-wifi-disconnect').focus(); };
  $('cancel-wifi-disconnect').onclick = () => { $('wifi-disconnect-confirm').hidden = true; $('disconnect-wifi').focus(); };
  $('confirm-wifi-disconnect').onclick = async () => { if (await post('/api/wifi/disconnect')) $('wifi-disconnect-confirm').hidden = true; };
  $('show-hotspot-password').onchange = event => { $('hotspot-password').type = event.target.checked ? 'text' : 'password'; };
  const cancel = () => { confirmedHotspot = null; $('hotspot-confirm').hidden = true; };
  for (const id of ['hotspot-ssid', 'hotspot-password']) $(id).oninput = cancel;
  $('cancel-hotspot').onclick = () => { cancel(); $('save-hotspot').focus(); };
  $('hotspot-form').onsubmit = event => {
    event.preventDefault();
    try {
      confirmedHotspot = hotspotChange($('hotspot-ssid').value, $('hotspot-password').value);
      if (confirmedHotspot.ssid === data.wifi.ssid && !confirmedHotspot.password) { message('hotspot-message', '热点设置没有变化。'); return; }
      text('hotspot-confirm-text', `修改后请重新连接“${confirmedHotspot.ssid}”${confirmedHotspot.password ? '，并使用新密码' : '，密码保持不变'}。`);
      $('hotspot-confirm').hidden = false; $('confirm-hotspot').focus();
    } catch (error) { message('hotspot-message', error.message, true); }
  };
  $('confirm-hotspot').onclick = async () => {
    if (posting || !confirmedHotspot) return;
    posting = true; render();
    try {
      const result = await api('/api/wifi/hotspot', confirmedHotspot);
      rememberHotspot({ ssid: result.ssid, jobId: result.job_id, bootId: data.app.boot_id });
      cancel(); $('hotspot-password').value = ''; $('show-hotspot-password').checked = false; $('hotspot-password').type = 'password';
      $('hotspot-message').hidden = true;
      await refresh(true);
    } catch (error) { message('hotspot-message', error.message, true); }
    finally { posting = false; render(); }
  };
  $('check-hotspot').onclick = () => refresh(true);
}
function renderMapFields(region) {
  const provider = mapElement(region, 'provider').value, info = providers[provider];
  const configured = !!data?.app.configured_providers?.[provider];
  mapElement(region, 'credentials').hidden = provider === 'osm'; mapElement(region, 'security-field').hidden = provider !== 'amap';
  mapElement(region, 'help').textContent = providerNotes[provider];
  mapElement(region, 'key-label').textContent = info.keyLabel || 'API Key';
  mapElement(region, 'state').textContent = configured ? '已配置' : '待配置';
  mapElement(region, 'key').placeholder = configured ? '已保存，留空沿用' : '粘贴 Key';
  mapElement(region, 'security-code').placeholder = configured ? '已保存，留空沿用' : '粘贴同组安全密钥';
  mapElement(region, 'console').hidden = !info.console;
  if (info.console) mapElement(region, 'console').href = info.console;
}
function setupMaps() {
  for (const region of mapRegions) {
    mapElement(region, 'provider').onchange = () => {
      mapElement(region, 'key').value = ''; mapElement(region, 'security-code').value = '';
      mapElement(region, 'message').hidden = true; renderMapFields(region);
    };
    mapElement(region, 'form').onsubmit = async event => {
    event.preventDefault();
    try {
      const provider = mapElement(region, 'provider').value, target = `map-${region}-message`;
      const value = mapChange(provider, mapElement(region, 'key').value, mapElement(region, 'security-code').value, !!data?.app.configured_providers?.[provider], region);
      if (await post('/api/maps/config', value, target)) {
        mapElement(region, 'key').value = ''; mapElement(region, 'security-code').value = ''; renderMapFields(region);
        message(target, (region === 'domestic' ? '国内' : '国外') + '地图已设为' + providers[provider].label + '。');
      }
    } catch (error) { message(`map-${region}-message`, error.message, true); }
    };
  }
  setupFavoriteBackup();
}
function setupFavoriteBackup() {
  $('export-favorites').onclick = () => {
    download({ version: 2, favorites: data.app.favorites.map(storedPlace) }, `移点盒收藏-${dateName()}.json`);
    message('backup-message', '收藏已导出。');
  };
  $('import-favorites').onclick = () => $('import-file').click();
  $('import-file').onchange = async event => {
    const file = event.target.files?.[0]; event.target.value = ''; if (!file) return;
    if (file.size > 16384) { message('backup-message', '备份文件最多16KB，请选择盒子导出的 JSON 文件。', true); return; }
    let value;
    try { value = JSON.parse(await file.text()); } catch { message('backup-message', '文件不是有效的 JSON 收藏备份。', true); return; }
    await post('/api/favorites/import', value, 'backup-message');
  };
}

async function exportDeviceBackup() {
  if (deviceBackup) {
    download(deviceBackup, `移点盒完整备份-${dateName()}.waypinbackup`);
    deviceBackup = null;
    message('device-backup-message', '备份文件已提交下载。');
    renderDeviceBackup();
    return;
  }
  backupPending = true;
  renderDeviceBackup();
  try {
    const { token, expires_in } = await api('/api/backup/export', {});
    const deadline = Date.now() + expires_in * 1000;
    message('device-backup-message', `请在 ${expires_in} 秒内按住盒子的 BOOT 键至少 6 秒，直到页面提示保存。`);
    while (Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 850));
      const result = await api('/api/backup/export', { token }, { binary: true });
      if (result instanceof Blob) {
        deviceBackup = result;
        message('device-backup-message', '备份已就绪，请点击保存备份文件。');
        return;
      }
    }
    throw new Error('备份确认已过期，请重新导出。');
  } catch (error) {
    message('device-backup-message', error.message, true);
  } finally {
    backupPending = false;
    renderDeviceBackup();
  }
}
function manualPlace() {
  const point = parseCoordinates($('manual-coordinates').value);
  return makePlace($('manual-name').value.trim() || '临时地点', point.latitude, point.longitude, { source: 'manual' });
}
function setupLocation() {
  $('clear-events').onclick = () => post('/api/events/clear');
  $('replay-location').onclick = () => post('/api/replay');
  $('manual-form').onsubmit = async event => {
    event.preventDefault();
    try { await post('/api/set', manualPlace(), 'manual-message'); } catch (error) { message('manual-message', error.message, true); }
  };
  $('manual-save').onclick = async () => {
    try { if (await post('/api/favorites', manualPlace(), 'manual-message')) message('manual-message', '收藏已保存。'); } catch (error) { message('manual-message', error.message, true); }
  };
}
$('refresh-admin').onclick = () => refresh(true);
$('admin-restore').onclick = () => post('/api/restore');
$('export-diagnostics').onclick = () => {
  const { app, wifi, debug, device } = data;
  download({ version: 1, captured_at: new Date().toISOString(), preview: !!app.preview, online,
    device, app: { version: app.version, uptime: app.uptime, mode: app.mode, paired: app.paired, maps_configured: app.maps_configured, map_provider: app.map_provider, operation: app.operation, restore_queued: app.restore_queued, network_busy: app.network_busy, button: app.button, result: app.result, favorites_count: app.favorites.length, recent_count: app.recent.length },
    wifi: { ssid: wifi.ssid, address: wifi.address, channel: wifi.channel, client_count: wifi.client_count, local_only: wifi.local_only, upstream: wifi.upstream }, events: debug.events
  }, `移点盒诊断-${dateName()}.json`);
  toast('诊断已导出，不含配对资料、地图凭证或 Wi-Fi 密码。');
};
({ network: setupNetwork, maps: setupMaps, location: setupLocation }[page])?.();
if (page === 'help') $('export-device-backup').onclick = exportDeviceBackup;
for (const icon of document.querySelectorAll('[data-lucide]')) if (icons[icon.dataset.lucide]) icon.innerHTML = icons[icon.dataset.lucide];
$('admin-page-select').value = location.pathname;
$('admin-page-select').onchange = event => location.assign(event.target.value);
for (const link of document.querySelectorAll('.admin-nav a')) if (link.pathname === location.pathname) link.setAttribute('aria-current', 'page');
document.addEventListener('visibilitychange', () => { clearTimeout(timer); if (!document.hidden) refresh(true); });
window.addEventListener('online', () => refresh(true));
render(); refresh();
