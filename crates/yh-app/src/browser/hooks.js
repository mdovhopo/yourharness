(() => {
  if (window.__yhHooked) return;
  window.__yhHooked = true;
  const handler = window.webkit?.messageHandlers?.yourHarnessLog;
  if (!handler) return;
  const post = (m) => { try { handler.postMessage(m); } catch {} };
  const fmt = (a) => {
    try {
      if (typeof a === 'string') return a;
      if (a instanceof Error) return a.stack ? String(a) + '\n' + a.stack : String(a);
      if (typeof Node !== 'undefined' && a instanceof Node) return '<' + a.nodeName.toLowerCase() + '>';
      if (a === undefined) return 'undefined';
      if (typeof a === 'function') return 'ƒ ' + (a.name || 'anonymous');
      const s = JSON.stringify(a);
      return s === undefined ? String(a) : s.length > 2000 ? s.slice(0, 2000) + '…' : s;
    } catch { return String(a); }
  };
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const original = console[level];
    console[level] = function (...args) {
      post({ kind: 'console', level, text: args.map(fmt).join(' ').slice(0, 8000) });
      return original.apply(this, args);
    };
  }
  addEventListener('error', (e) => {
    const t = e.target;
    if (t && t !== window) {
      // A resource that didn't load: an image, script, stylesheet, video.
      const url = String(t.currentSrc || t.src || t.href || t.data || '');
      // An empty or "#" source resolves to the page itself: not a request that failed.
      if (url && url.split('#')[0] !== location.href.split('#')[0]) post({ kind: 'request', method: 'GET', url, failure: 'Failed to load', type: t.localName });
    } else if (e instanceof ErrorEvent) {
      const where = e.filename ? ' (' + e.filename + ':' + e.lineno + ')' : '';
      post({ kind: 'console', level: 'error', text: 'Uncaught ' + (e.error ? fmt(e.error) : e.message) + where });
    }
  }, true);
  addEventListener('unhandledrejection', (e) => post({ kind: 'console', level: 'error', text: 'Unhandled rejection: ' + fmt(e.reason) }));

  const originalFetch = window.fetch;
  if (originalFetch) window.fetch = async function (input, init) {
    const start = performance.now();
    const method = String(init?.method || input?.method || 'GET').toUpperCase();
    const url = String(input?.url || input);
    try {
      const response = await originalFetch.apply(this, arguments);
      post({ kind: 'request', method, url: response.url || url, status: response.status, type: 'fetch', ms: performance.now() - start });
      return response;
    } catch (error) {
      if (error?.name !== 'AbortError') post({ kind: 'request', method, url, failure: String(error?.message || error), type: 'fetch', ms: performance.now() - start });
      throw error;
    }
  };
  const open = XMLHttpRequest.prototype.open, send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__yh = { method: String(method).toUpperCase(), url: String(url) };
    return open.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function () {
    const info = this.__yh, start = performance.now();
    if (info) this.addEventListener('loadend', () => post({
      kind: 'request', method: info.method, url: this.responseURL || info.url, status: this.status || undefined,
      failure: this.status === 0 ? 'Network error' : undefined, type: 'xhr', ms: performance.now() - start }));
    return send.apply(this, arguments);
  };
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (e.initiatorType === 'fetch' || e.initiatorType === 'xmlhttprequest') continue;
        post({ kind: 'request', method: 'GET', url: e.name, status: e.responseStatus || undefined, type: e.initiatorType, ms: e.duration });
      }
    }).observe({ type: 'resource', buffered: true });
  } catch {}
})();
