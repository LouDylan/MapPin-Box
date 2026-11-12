import { makePlace, samePosition, storedPlace } from './coordinates.mjs';
import { icons } from './lucide-icons.mjs';
import { createPlacePicker } from './place-picker.mjs';
import { api } from './api.mjs';
import { resolvePlace } from './places.mjs';

const $ = id => document.getElementById(id);

let state = null, online = false, loading = true, failures = 0, pollTimer = null, pollPromise = null;
let postingSet = false, postingRestore = false, editing = null, formPosting = false;
let snapshotAt = performance.now(), listSignature = '', lastResult = null, toastTimer = null;
let picker = null, editingPlace = null, savingSelection = false, lastButtonId = null, resolving = false, switchingMap = false;

function text(id, value) { if ($(id).textContent !== value) $(id).textContent = value; }
function element(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}
function icon(name, className = '') {
  const node = element('span', className);
  node.innerHTML = icons[name];
  return node;
}
function toast(message, error = false) {
  clearTimeout(toastTimer);
  text('toast', message); $('toast').dataset.error = String(error); $('toast').hidden = false;
  toastTimer = setTimeout(() => { $('toast').hidden = true; }, error ? 7000 : 4000);
}
function busy() { return resolving || postingSet || !!state?.operation || !!state?.network_busy; }
function ready() { return online && !!state?.paired; }

function schedulePoll() {
  clearTimeout(pollTimer);
  if (document.hidden) return;
  const delay = online ? (state?.operation ? 900 : 3000) : Math.min(10000, 1500 * 2 ** Math.min(failures, 3));
  pollTimer = setTimeout(() => refresh(), delay);
}
async function refresh(force = false) {
  if (pollPromise) { await pollPromise; return force ? refresh() : undefined; }
  clearTimeout(pollTimer);
  pollPromise = (async () => {
    try {
      const next = await api('/api/status');
      const newBoot = state && next.boot_id !== state.boot_id;
      if (!state || newBoot) { lastResult = next.result?.id ?? null; lastButtonId = next.button?.id ?? null; }
      if (next.button && next.button.id !== lastButtonId) {
        lastButtonId = next.button.id;
        if (!next.button.ok) toast(next.button.message, true);
      }
      state = next; online = true; failures = 0; snapshotAt = performance.now();
      if (next.result && next.result.id !== lastResult) {
        lastResult = next.result.id; toast(next.result.message, !next.result.ok);
      }
    } catch { online = false; failures += 1; }
    loading = false;
    render();
  })();
  try { await pollPromise; } finally { pollPromise = null; schedulePoll(); }
}
function render() {
  text('connection-label', loading ? '连接中' : online ? '在线' : '离线');
  $('connection').dataset.online = String(online);
  $('preview-label').hidden = !state?.preview; $('preview-notice').hidden = !state?.preview;
  $('offline-banner').hidden = loading || (online && state?.paired);
  text('offline-text', online ? '先接入 iPhone，再使用定位。' : '暂时无法连接盒子，请检查供电和 Wi-Fi。');
  text('reconnect', online ? '接入 iPhone' : '重新连接');
  const op = online ? state?.operation : null;
  let mode = 'unknown', title = '定位状态待确认', label = '待确认', caption = '';
  if (loading) { title = '正在连接'; label = '读取中'; caption = ''; }
  else if (!online) { caption = '请重新连接盒子，再查看定位状态。'; }
  else if (!state.paired) { title = '尚未接入 iPhone'; label = '未接入'; caption = ''; }
  else if (state.network_busy) { mode = 'working'; title = '正在调整 Wi-Fi'; label = '维护中'; caption = ''; }
  else if (op) { mode = 'working'; title = op.kind === 'restore' ? '正在恢复真实定位' : '正在设置：' + op.label; label = op.phase === 'queued' ? '已排队' : '执行中'; caption = state.restore_queued ? '当前操作结束后恢复真实定位。' : state.preview ? '正在演示执行流程，不会修改手机位置。' : '保持 iPhone 解锁并与盒子连接。'; }
  else if (state.mode === 'simulating' && state.location) { mode = 'simulating'; title = state.location.name; label = '已设置'; caption = state.location.address || ''; }
  else if (state.mode === 'real') { mode = 'real'; title = '真实定位'; label = '已恢复'; caption = ''; }
  else if (state.result && !state.result.ok) { caption = '上次操作未完成，请按下方提示重试。'; }
  $('position-card').dataset.mode = mode;
  $('position-card').hidden = online && !state?.paired;
  text('current-title', title); text('mode-label', label); text('current-caption', caption); $('current-caption').hidden = !caption;
  $('activity').hidden = !op;
  if (op) text('activity-label', op.kind === 'restore' ? '恢复真实定位' : '连接并设置位置');
  updateElapsed();
  const failed = online && state?.result && !state.result.ok && !op;
  $('error-panel').hidden = !failed;
  if (failed) { text('error-message', state.result.message); text('error-detail', state.result.detail || '请保持 iPhone 解锁并与盒子连接后重试。'); }
  const restoring = state?.restore_queued || (op && op.kind === 'restore') || postingRestore;
  $('restore').disabled = !ready() || !!restoring || !!state?.network_busy;
  text('restore-label', state?.restore_queued ? '恢复已排队' : restoring ? '正在恢复…' : '恢复真实定位');

  text('favorite-count', String(state?.favorites.length || 0));
  text('recent-count', String(state?.recent.length || 0));
  $('clear-history').hidden = !state?.recent.length;
  $('clear-history').disabled = !online;
  $('search-wrap').hidden = (state?.favorites.length || 0) < 5 && !$('search').value;
  renderLists(); renderDialogControls();
  picker?.update({ online, paired: !!state?.paired, busy: busy(), favorites: state?.favorites || [], operation: op,
    provider: state?.map_provider, regionalProviders: state?.regional_providers, switchingMap, mode: state?.mode, location: state?.location, restoreQueued: state?.restore_queued, preview: state?.preview, saving: savingSelection });
}
function updateElapsed() {
  if (!state?.operation || !online) return;
  const elapsed = Math.floor((state.operation.elapsed_ms + performance.now() - snapshotAt) / 1000);
  text('elapsed', `${elapsed} 秒`);
}
function emptyState(title, hint) {
  const box = element('div', 'empty-state');
  box.append(element('strong', '', title));
  if (hint) box.append(element('p', '', hint));
  return box;
}

function renderLists() {
  if (!state) {
    if (!loading) { $('favorites-list').replaceChildren(emptyState('收藏暂时无法读取', '')); $('recent-list').replaceChildren(); }
    return;
  }
  const filter = $('search').value.trim().toLocaleLowerCase();
  const signature = JSON.stringify([state.favorites, state.recent, state.mode, state.location, online, busy(), state.paired, filter]);
  if (signature === listSignature) return;
  listSignature = signature;
  const favorites = state.favorites.filter(place => place.name.toLocaleLowerCase().includes(filter));
  const favoriteNodes = favorites.map(place => {
    const card = element('div', `place-card${state.mode === 'simulating' && online && samePosition(place, state.location) ? ' active' : ''}`);
    const button = element('button', 'place-apply'); button.type = 'button'; button.disabled = !ready() || busy();
    button.setAttribute('aria-label', `切换到${place.name}`); button.onclick = () => useSavedPlace(place);
    const copy = element('span', 'place-copy'); copy.append(element('span', 'place-name', place.name), element('span', 'place-address', place.address || (place.source === 'google' ? 'Google 地图' : '')));
    button.append(icon('map-pin','place-marker'), copy);
    if (state.mode === 'simulating' && online && samePosition(place, state.location)) button.append(element('span', 'active-tag', '使用中'));
    const edit = element('button', 'edit-place'); edit.type = 'button'; edit.setAttribute('aria-label', `编辑收藏${place.name}`); edit.append(icon('pencil')); edit.onclick = () => openPlace(place, place.id);
    card.append(button, edit); return card;
  });
  if (!favoriteNodes.length) favoriteNodes.push(filter ? emptyState('没有找到这个地点', '试试其他名称。') : emptyState('暂无收藏', ''));
  $('favorites-list').replaceChildren(...favoriteNodes);
  const recentNodes = state.recent.map(place => {
    const row = element('div', 'recent-row');
    const button = element('button', 'recent-apply'); button.type = 'button'; button.disabled = !ready() || busy();
    button.setAttribute('aria-label', `再次使用${place.name}`); button.onclick = () => useSavedPlace(place);
    const copy = element('span', 'place-copy'); copy.append(element('span', 'place-name', place.name), element('span', 'place-address', place.address || (place.source === 'google' ? 'Google 地图' : '')));
    button.append(copy);
    const saved = state.favorites.some(item => samePosition(item, place));
    const star = element('button', `favorite-star${saved ? ' saved' : ''}`); star.type = 'button'; star.disabled = saved || !online;
    star.setAttribute('aria-label', saved ? `${place.name}已收藏` : `收藏${place.name}`); star.append(icon('star')); star.onclick = () => quickSave(place);
    row.append(button, star); return row;
  });
  if (!recentNodes.length) recentNodes.push(emptyState('暂无记录', ''));
  $('recent-list').replaceChildren(...recentNodes);
}
async function gps(kind, place) {
  if (!ready()) { toast('请先连接盒子 Wi-Fi，并完成 iPhone 配对', true); return false; }
  if (kind === 'set' && busy()) { toast('正在执行上一项操作，请稍候'); return false; }
  if (kind === 'restore') postingRestore = true; else postingSet = true;
  render();
  try {
    await api(kind === 'set' ? '/api/set' : '/api/restore', place || {});
    await refresh(true);
    return true;
  } catch (error) { toast(error.message, true); await refresh(true); return false; }
  finally { if (kind === 'set') postingSet = false; else postingRestore = false; render(); }
}
function apply(place) { return gps('set', makePlace(place.name, place.latitude, place.longitude, place)); }
async function useSavedPlace(place) {
  if (busy()) return;
  resolving = true; render();
  try {
    const resolved = await resolvePlace(place);
    picker.select(resolved, { scroll: false });
    resolving = false;
    await apply(resolved);
  } catch (error) { toast(error.message, true); }
  finally { resolving = false; render(); }
}
async function quickSave(place) {
  if (savingSelection) return;
  savingSelection = true; render();
  try { await api('/api/favorites', storedPlace(place)); toast('已加入常用地点'); await refresh(true); }
  catch (error) { toast(error.message, true); }
  finally { savingSelection = false; render(); }
}
function openPlace(place, id) {
  if (formPosting) { toast('正在保存地点，请稍候'); return; }
  editing = id; editingPlace = place;
  $('place-form').reset(); $('form-error').hidden = true;
  $('place-name').removeAttribute('aria-invalid');
  text('saved-place-address', place.address || ''); $('saved-place-address').hidden = !place.address;
  $('place-name').value = place.name;
  $('place-dialog').showModal(); renderDialogControls();
  $('place-name').focus(); $('place-name').select();
}
function renderDialogControls() {
  $('save-place').disabled = formPosting || !online;
  $('delete-place').disabled = formPosting || !online;
}
function formError(error) {
  text('form-error', error.message); $('form-error').hidden = false;
  const input = $(`place-${error.field}`);
  if (input) { input.setAttribute('aria-invalid', 'true'); input.focus(); }
}
async function savePlace(event) {
  event.preventDefault(); if (formPosting) return;
  let place;
  try { place = storedPlace(makePlace($('place-name').value, editingPlace.latitude, editingPlace.longitude, { ...editingPlace, ...(editingPlace.source === 'google' ? { query: $('place-name').value.trim() } : {}) })); }
  catch (error) { formError(error); return; }
  formPosting = true; renderDialogControls();
  try {
    await api('/api/favorites', { ...place, id: editing });
    $('place-dialog').close(); toast(editing ? '收藏已更新' : '收藏已保存'); await refresh(true);
  } catch (error) { formError(error); }
  finally { formPosting = false; renderDialogControls(); }
}
async function deletePlace() {
  if (!editing || !confirm(`删除收藏“${$('place-name').value}”？`)) return;
  formPosting = true; renderDialogControls();
  try { await api('/api/favorites/delete', { id: editing }); $('place-dialog').close(); toast('已删除收藏'); await refresh(true); }
  catch (error) { formError(error); }
  finally { formPosting = false; renderDialogControls(); }
}
$('close-dialog').onclick = () => $('place-dialog').close();
$('place-form').onsubmit = savePlace; $('delete-place').onclick = deletePlace;
$('place-form').addEventListener('input', event => event.target.removeAttribute('aria-invalid'));
$('search').addEventListener('input', renderLists);
$('restore').onclick = () => gps('restore');
$('reconnect').onclick = () => { if (online) { location.assign('/admin/help#iphone'); } else refresh(true); };
$('clear-history').onclick = async () => { if (!confirm('清空最近记录？BOOT 长按也将没有可用地点，直到下次定位成功。')) return; try { await api('/api/history/clear', {}); toast('最近使用记录已清空'); await refresh(true); } catch (error) { toast(error.message, true); } };
async function switchProvider(provider) {
  if (switchingMap || !online || provider === state?.map_provider) return;
  switchingMap = true; render();
  try {
    await api('/api/maps/config', { provider });
    await refresh(true);
    await picker.reloadService();
  } catch (error) { toast(error.message, true); }
  finally { switchingMap = false; render(); }
}
function openMapSettings() { location.assign('/admin/maps'); }
function showLibrary(which, focus = false) {
  const favorites = which === 'favorites';
  $('favorites-pane').hidden = !favorites; $('recent-pane').hidden = favorites;
  for (const name of ['favorites', 'recent']) {
    const tab = $(name + '-tab'), active = (name === which);
    tab.setAttribute('aria-selected', String(active)); tab.tabIndex = active ? 0 : -1;
    if (active && focus) tab.focus();
  }
}
for (const name of ['favorites', 'recent']) {
  $(name + '-tab').onclick = () => showLibrary(name);
  $(name + '-tab').onkeydown = event => {
    if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); showLibrary(event.key === 'Home' ? 'favorites' : event.key === 'End' ? 'recent' : name === 'favorites' ? 'recent' : 'favorites', true);
    }
  };
}
document.addEventListener('visibilitychange', () => { clearTimeout(pollTimer); if (!document.hidden) refresh(true); });
window.addEventListener('online', () => refresh(true));
setInterval(updateElapsed, 1000);
for (const node of document.querySelectorAll('[data-lucide]')) {
  if (icons[node.dataset.lucide]) node.innerHTML = icons[node.dataset.lucide];
}
picker = createPlacePicker({ onApply: apply, onSave: quickSave, onConfigureMap: openMapSettings, onSwitchProvider: switchProvider });
render(); refresh();
