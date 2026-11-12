// One Google SDK per document. Authorization failures belong to its whole
// lifetime, including the interval before a map has been constructed.
let pending = null, activeKey = null, authError = null, rejectLoad = null, attempt = 0;
const listeners = new Set();
const failure = code => Object.assign(new Error(code), { code });

export function observeGoogleAuthFailure(listener) {
  listeners.add(listener);
  if (authError) listener(authError);
  return () => listeners.delete(listener);
}

function unauthorized() {
  if (authError) return;
  authError = failure('GOOGLE_AUTH_FAILED');
  rejectLoad?.(authError);
  for (const listener of listeners) listener(authError);
}

function start(key) {
  return new Promise((resolve, reject) => {
    let node = null, settled = false;
    const callback = '__i4GoogleReady' + ++attempt;
    const finish = (error, api) => {
      if (settled) return;
      settled = true; clearTimeout(timer); rejectLoad = null;
      if (node) { node.onerror = null; if (error) node.remove(); }
      // A downloaded SDK may still invoke its callback after authorization fails.
      window[callback] = () => {};
      if (error) reject(error); else resolve(api);
    };
    const timer = setTimeout(() => finish(failure('GOOGLE_SDK_TIMEOUT')), 15000);
    rejectLoad = error => finish(error);
    const ready = async () => {
      try {
        const api = window.google?.maps;
        if (!api?.importLibrary) throw failure('GOOGLE_SDK_UNAVAILABLE');
        await Promise.all([api.importLibrary('maps'), api.importLibrary('places')]);
        if (authError) throw authError;
        finish(null, api);
      } catch (error) { finish(error); }
    };
    if (window.google?.maps?.importLibrary) { ready(); return; }
    window[callback] = ready;
    const url = new URL('https://maps.googleapis.com/maps/api/js');
    url.search = new URLSearchParams({ key, v: 'quarterly', loading: 'async', callback, language: 'zh-CN' });
    node = document.createElement('script'); node.async = true; node.src = url.href;
    node.onerror = () => finish(failure('GOOGLE_SDK_NETWORK'));
    document.head.append(node);
  });
}

export function loadGoogleSdk(key) {
  if (activeKey && activeKey !== key) return Promise.reject(failure('GOOGLE_KEY_CHANGED'));
  if (authError) return Promise.reject(authError);
  if (pending) return pending;
  activeKey = key;
  window.gm_authFailure = unauthorized;
  const loading = start(key).catch(error => { if (pending === loading) pending = null; throw error; });
  pending = loading;
  return loading;
}
