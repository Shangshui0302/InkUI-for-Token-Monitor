// Coalesce changes until the next frame. Native waits for the WebView compositor.
export function createDisplayRefresh(bridge, hidden, frame = requestAnimationFrame, cancelFrame = cancelAnimationFrame) {
  const priority = { content: 1, page: 2, full: 3 };
  let pending = null, scheduled = null;
  return {
    request(reason = 'content') {
      if (!priority[reason] || hidden()) return;
      if (!pending || priority[reason] > priority[pending]) pending = reason;
      if (scheduled !== null) return;
      scheduled = frame(() => {
        scheduled = null;
        const next = pending; pending = null;
        if (!hidden() && next) {
          try { bridge.requestDisplayRefresh?.(next); } catch (_) { /* Older APKs use system refresh. */ }
        }
      });
    },
    cancel() { if (scheduled !== null) cancelFrame(scheduled); scheduled = null; pending = null; }
  };
}
