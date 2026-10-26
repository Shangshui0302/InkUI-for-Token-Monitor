/* ES5 bootstrap: it must still run when the module bundle cannot be parsed. */
(function () {
  'use strict';
  var failed = false, ready = false;
  window.TokenMonitorBoot = { ready: function () { if (failed) return false; ready = true; return true; } };
  function fail(category) {
    if (failed || ready) return;
    failed = true;
    try {
      var bridge = window.TokenMonitorAndroid;
      if (bridge && typeof bridge.uiFailed === 'function') bridge.uiFailed(category);
    } catch (_) { /* Native startup deadline remains the final fallback. */ }
  }
  window.addEventListener('error', function (event) {
    var target = event.target;
    if (target && target.tagName === 'SCRIPT') fail('module_load_error');
    else fail('bootstrap_error');
  }, true);
  window.addEventListener('unhandledrejection', function () { fail('bootstrap_error'); });
  var script = document.createElement('script');
  var element = document.documentElement;
  if (!('noModule' in script) || typeof window.fetch !== 'function'
      || typeof window.AbortController !== 'function' || typeof window.Promise !== 'function'
      || typeof window.URL !== 'function' || typeof window.Map !== 'function'
      || typeof window.Set !== 'function' || typeof window.queueMicrotask !== 'function'
      || typeof window.requestAnimationFrame !== 'function' || typeof window.CustomEvent !== 'function'
      || typeof element.replaceChildren !== 'function' || typeof element.toggleAttribute !== 'function'
      || typeof element.closest !== 'function' || typeof Object.entries !== 'function'
      || typeof Object.values !== 'function' || typeof Object.fromEntries !== 'function'
      || typeof Object.hasOwn !== 'function'
      || typeof Array.from !== 'function' || typeof Array.prototype.flatMap !== 'function'
      || typeof Array.prototype.includes !== 'function' || typeof String.prototype.padStart !== 'function'
      || typeof Math.imul !== 'function'
      || !window.CSS || typeof window.CSS.supports !== 'function'
      || !window.CSS.supports('display', 'grid')) fail('unsupported_webview');
}());
