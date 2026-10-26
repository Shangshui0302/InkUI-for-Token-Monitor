// Coalesce changes until the next frame. Native waits for the WebView compositor.
export function createDisplayRefresh(bridge, hidden, frame = requestAnimationFrame, cancelFrame = cancelAnimationFrame) {
  const priority = { content: 1, page: 2, full: 3 };
  let pending = null, scheduled = null, generation = 0;
  return {
    request(reason = 'content') {
      if (typeof priority[reason] !== 'number' || hidden()) return;
      if (!pending || priority[reason] > priority[pending]) pending = reason;
      if (scheduled !== null) return;
      const ticket = generation;
      scheduled = frame(() => {
        if (ticket !== generation) return;
        scheduled = null;
        const next = pending; pending = null;
        if (!hidden() && next) {
          try { bridge.requestDisplayRefresh?.(next); } catch (_) { /* Older APKs use system refresh. */ }
        }
      });
    },
    cancel() { generation++; if (scheduled !== null) cancelFrame(scheduled); scheduled = null; pending = null; }
  };
}
