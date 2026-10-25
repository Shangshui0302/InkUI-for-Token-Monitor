// Recognize a page turn on release; never translate or scroll the content.
export function swipeDirection(start, end, width) {
  if (!start || !end) return 0;
  const dx = end.x - start.x, dy = end.y - start.y, elapsed = end.time - start.time;
  const threshold = Math.max(32, Math.min(64, width * .18));
  return elapsed >= 0 && elapsed <= 1000 && Math.abs(dx) >= threshold && Math.abs(dx) >= Math.abs(dy) * 2
    ? dx < 0 ? 1 : -1 : 0;
}

export function installPageSwipe(root, turn, enabled = () => true) {
  let start = null, moved = false, vertical = false, suppressUntil = 0;
  const pointers = new Set();
  const point = event => ({ x: event.clientX, y: event.clientY, time: event.timeStamp });
  const cancel = () => { start = null; moved = false; vertical = false; };
  const down = event => {
    if (event.pointerType === 'mouse') { suppressUntil = 0; return; }
    if (event.pointerType !== 'touch' && event.pointerType !== 'pen') return;
    pointers.add(event.pointerId);
    if (pointers.size > 1) { cancel(); suppressUntil = Date.now() + 700; return; }
    suppressUntil = 0;
    const bounds = root.getBoundingClientRect();
    // Leave screen edges to Android's back gesture; preserve caret/selection in forms.
    if (!event.isPrimary || !enabled() || !root.contains(event.target) || event.target.closest('input,textarea,select,[contenteditable]')
        || event.clientX < 20 || event.clientX > root.ownerDocument.documentElement.clientWidth - 20) return;
    start = { ...point(event), id: event.pointerId, width: bounds.width };
    moved = false;
  };
  const move = event => {
    if (!start || event.pointerId !== start.id) return;
    moved ||= Math.hypot(event.clientX - start.x, event.clientY - start.y) > 12;
    if (Math.abs(event.clientY - start.y) > 24 && Math.abs(event.clientY - start.y) > Math.abs(event.clientX - start.x)) vertical = true;
    if (event.cancelable) event.preventDefault();
  };
  const up = event => {
    pointers.delete(event.pointerId);
    if (!start || event.pointerId !== start.id) return;
    const direction = vertical ? 0 : swipeDirection(start, point(event), start.width);
    const dragged = moved || Math.hypot(event.clientX - start.x, event.clientY - start.y) > 12;
    cancel();
    if (dragged) {
      suppressUntil = Date.now() + 700;
      if (event.cancelable) event.preventDefault();
    }
    if (direction && enabled()) turn(direction);
  };
  const aborted = event => { pointers.delete(event.pointerId); cancel(); suppressUntil = Date.now() + 700; };
  const click = event => {
    // A dragged quota card must not open its detail after the page has changed.
    if (event.detail && Date.now() < suppressUntil) { event.preventDefault(); event.stopImmediatePropagation(); }
  };
  const reset = () => { pointers.clear(); cancel(); };
  const doc = root.ownerDocument;
  doc.addEventListener('pointerdown', down, true);
  doc.addEventListener('pointermove', move, { passive: false });
  doc.addEventListener('pointerup', up);
  doc.addEventListener('pointercancel', aborted);
  doc.addEventListener('click', click, true);
  doc.addEventListener('visibilitychange', reset);
  return reset;
}
