import { loadGoogleSdk, observeGoogleAuthFailure } from './google-sdk.mjs';

export class MapServiceError extends Error {
  constructor(message, action = 'retry') { super(message); this.action = action; }
}
export const providers = Object.freeze({
  amap: { label: '高德地图', credit: '地点与地图 © 高德地图', console: 'https://console.amap.com/', keyLabel: 'Web端（JS API）Key', help: '平台选择 Web端（JS API），填写 Key 和对应的安全密钥。' },
  osm: { label: '开源地图', credit: '底图 OpenFreeMap / OpenMapTiles / OpenStreetMap · 搜索 Photon / Wikidata', help: 'OpenFreeMap 全球底图与 Photon 搜索，无需密钥。中文地标由 Wikidata 补充。' },
  google: { label: 'Google 地图', credit: 'Google Maps', console: 'https://console.cloud.google.com/google/maps-apis/credentials', keyLabel: 'Google Maps API Key', help: '在 Key 所属项目启用 Maps JavaScript API、Places API (New) 并关联有效结算账号。应用限制使用 Websites，允许 http://192.168.71.1/*；API 限制列表不代替服务启用。Google 收藏使用时需联网，BOOT 重用限本次定位后10分钟。' },
  tianditu: { label: '天地图', credit: '© 天地图', console: 'https://console.tianditu.gov.cn/', keyLabel: '浏览器端 Key', help: '在天地图控制台创建浏览器端应用，启用 JavaScript API 与搜索服务。' },
});
export const regionalSources = Object.freeze({ domestic: ['amap', 'tianditu'], global: ['osm', 'google'] });
export const providerNotes = Object.freeze({ amap: '填写 Web端 Key 和安全密钥。', tianditu: '填写浏览器端 Key。', osm: '无需 Key。', google: '需要能访问 Google 服务的网络。' });
const valid = text => /^[a-z\d_-]{16,128}$/iu.test(text);
export function validateMapConfig(value) {
  const key = String(value?.key || '').trim(), security_code = String(value?.security_code || '').trim();
  if (![key, security_code].every(valid)) throw new MapServiceError('请填写同一组高德 Web端（JS API）Key 和安全密钥。', 'configure');
  return { key, security_code };
}
export function mapChange(provider, key, security_code, configured = false, region = null) {
  if (!providers[provider]) throw new MapServiceError('请选择地图服务', 'configure');
  if (region !== null && !regionalSources[region]?.includes(provider)) throw new MapServiceError('地图源与所选地区不匹配', 'configure');
  const choice = region === null ? { provider } : { provider, region };
  key = String(key || '').trim(); security_code = String(security_code || '').trim();
  if (provider === 'osm' || (configured && !key && !security_code)) return choice;
  if (provider === 'amap') return { ...choice, ...validateMapConfig({ key, security_code }) };
  if (!valid(key)) throw new MapServiceError('请填写所选服务的 API Key', 'configure');
  return { ...choice, key };
}
// Raw provider errors and URLs may contain credentials; only show fixed messages.
export function mapServiceError(error, provider = 'amap') {
  if (error instanceof MapServiceError || error?.name === 'AbortError') return error;
  const label = providers[provider]?.label || '地图服务';
  const code = [error?.code, error?.info, error?.message, error].filter(Boolean).join(' ').toUpperCase();
  if (provider === 'google' && /GOOGLE_KEY_CHANGED/u.test(code))
    return new MapServiceError('Google Key 已更新，请刷新网页后重试。', 'reload');
  if (provider === 'google' && /BILLING/u.test(code))
    return new MapServiceError('Google 地图的项目尚未启用有效结算，请在 Google Cloud 关联结算账号。', 'configure');
  if (provider === 'google' && /APINOTACTIVATED|SERVICE_DISABLED/u.test(code))
    return new MapServiceError('Google 地图服务尚未启用，请在 Key 所属项目启用 Maps JavaScript API 和 Places API (New)。', 'configure');
  if (provider === 'google' && /APITARGETBLOCKED/u.test(code))
    return new MapServiceError('Google Key 的 API 限制未允许当前服务，请允许 Maps JavaScript API 和 Places API (New)。', 'configure');
  if (provider === 'google' && /GOOGLE_AUTH_FAILED|PERMISSION_DENIED/u.test(code))
    return new MapServiceError('Google 地图授权失败，请检查结算、服务启用情况和 Websites 网站限制。', 'configure');
  if (provider === 'google' && /GOOGLE_SDK_TIMEOUT|MAP TIMEOUT/u.test(code))
    return new MapServiceError('Google 服务连接超时。', 'openMap');
  if (provider === 'google' && /GOOGLE_SDK_NETWORK/u.test(code))
    return new MapServiceError('当前网络无法连接 Google 服务。', 'openMap');
  if (/INVALID_USER_DOMAIN|INVALID_USER_REFERER|REFERERNOTALLOWED|REFERER_NOT_ALLOWED/u.test(code))
    return new MapServiceError(label + '拒绝了当前网页地址，请检查控制台中的网站限制。', 'configure');
  if (/INVALID_USER_KEY|INVALID_KEY|INVALID_USER_SCODE|INVALID_USER_SIGNATURE|USERKEY_PLAT_NOMATCH|API_KEY_INVALID|INVALIDKEY|REQUEST_DENIED|BILLING|APINOTACTIVATED/u.test(code))
    return new MapServiceError(label + '凭证未通过验证，请检查 Key 和服务权限。', 'configure');
  if (/LIMIT|QUOTA|TOO_FREQUENT|RESOURCE_EXHAUSTED|OVER_QUERY/u.test(code))
    return new MapServiceError(label + '调用额度或频率已达上限，请稍后重试。');
  if (/INSUFFICIENT_PRIVILEGES|SERVICE_NOT_AVAILABLE/u.test(code))
    return new MapServiceError(label + '尚未开通此服务，请检查账号认证和服务权限。', 'configure');
  return new MapServiceError('无法连接' + label + '，请检查互联网连接后重试。');
}
export function abortable(promise, signal) {
  if (!signal) return promise;
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise((resolve, reject) => {
    const cleanup = () => signal.removeEventListener('abort', abort);
    const abort = () => { cleanup(); reject(signal.reason); };
    signal.addEventListener('abort', abort, { once: true });
    promise.then(value => { cleanup(); resolve(value); }, error => { cleanup(); reject(error); });
  });
}
export function bounded(promise, milliseconds = 15000) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new MapServiceError('地图服务响应超时，请重试。')), milliseconds);
  })]).finally(() => clearTimeout(timer));
}
let configPromise = null, apiPromise = null;
observeGoogleAuthFailure(() => { apiPromise = null; });
export function resetMapService() { configPromise = null; apiPromise = null; }
export function readMapConfig() {
  if (!configPromise) { const pending = (async () => {
    const controller = new AbortController(), timer = setTimeout(() => controller.abort(), 6000);
    try {
      const response = await fetch('/api/maps/config', { cache: 'no-store', signal: controller.signal });
      if (!response.ok) throw new Error();
      const value = await response.json(), provider = value.provider || 'amap';
      if (!providers[provider]) throw new Error();
      if (provider === 'amap') validateMapConfig(value);
      else if (provider !== 'osm' && !valid(value.key || '')) throw new MapServiceError('请配置' + providers[provider].label, 'configure');
      return { ...value, provider };
    } catch (error) {
      if (error instanceof MapServiceError) throw error;
      throw new MapServiceError('无法读取地图配置，请重新连接盒子。');
    } finally { clearTimeout(timer); }
  })().catch(error => { if (configPromise === pending) configPromise = null; throw error; });
  configPromise = pending; }
  return configPromise;
}
function script(url, ready) {
  if (ready()) return Promise.resolve(ready());
  return new Promise((resolve, reject) => {
    const node = document.createElement('script');
    let poll;
    const cleanup = () => { clearInterval(poll); clearTimeout(timer); node.onload = node.onerror = null; };
    const fail = () => { cleanup(); node.remove(); reject(new Error('SDK unavailable')); };
    const timer = setTimeout(fail, 15000);
    const check = () => { if (ready()) { cleanup(); resolve(ready()); } };
    node.src = url; node.async = true; node.onload = check; node.onerror = fail;
    poll = setInterval(check, 80); document.head.append(node);
  });
}
async function apiFor(config) {
  if (config.provider === 'amap') {
    const loader = await script('https://webapi.amap.com/loader.js', () => window.AMapLoader);
    window._AMapSecurityConfig = { securityJsCode: config.security_code };
    return bounded(loader.load({ key: config.key, version: '2.0', plugins: ['AMap.PlaceSearch'] }));
  }
  if (config.provider === 'osm') {
    if (!document.querySelector('link[data-open-map]')) {
      const css = document.createElement('link'); css.rel = 'stylesheet'; css.href = '/assets/maplibre-gl.css'; css.dataset.openMap = ''; document.head.append(css);
    }
    return import('./assets/maplibre-gl.mjs');
  }
  if (config.provider === 'google') {
    return loadGoogleSdk(config.key);
  }
  const url = new URL('https://api.tianditu.gov.cn/api');
  url.search = new URLSearchParams({ v: '4.0', tk: config.key });
  return script(url.href, () => window.T?.Map && window.T?.LocalSearch && window.T);
}
export function loadMapService() {
  if (!apiPromise) { const pending = (async () => {
    const config = await readMapConfig();
    try { return { provider: config.provider, api: await apiFor(config), config }; }
    catch (error) { throw mapServiceError(error, config.provider); }
  })().catch(error => { if (apiPromise === pending) apiPromise = null; throw error; });
  apiPromise = pending; }
  return apiPromise;
}
export async function loadMapApi() { return (await loadMapService()).api; }
