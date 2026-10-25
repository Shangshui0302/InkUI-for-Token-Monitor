const DAY = 86_400_000;

// Parse calendar days in UTC rather than local time: DST must not change x spacing.
function dayNumber(value) {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const time = Date.parse(value + 'T00:00:00.000Z');
  if (!Number.isFinite(time) || new Date(time).toISOString().slice(0, 10) !== value) return null;
  return time / DAY;
}
const dateKey = day => new Date(day * DAY).toISOString().slice(0, 10);
const size = value => typeof value === 'number' && Number.isFinite(value) ? Math.max(0, value) : 0;
const coordinate = value => Object.is(value, -0) ? 0 : value;
const pair = (x, y) => `${coordinate(x)},${coordinate(y)}`;

/**
 * Pure, date-spaced SVG geometry. Missing dates/nulls are unknown, never zero.
 * Duplicate dates use the last row; invalid dates/negative or nonnumeric values
 * are ignored as observations. gaps contains only missing days BETWEEN observed
 * points, as {start, end, days}; leading/trailing unknown days remain blank.
 * inset accepts a pixel number or {top, right, bottom, left} (default 3 each).
 * line is continuous; segments and gaps retain missing-date metadata.
 * area is an optional visual envelope (including interpolated gaps), not a sum.
 */
export function trendGeometry(rows, { start, end, width, height, inset = 3 } = {}) {
  const first = dayNumber(start), last = dayNumber(end);
  const empty = { line: '', area: '', segments: [], points: [], gaps: [], missingDays: 0, max: 0 };
  if (first === null || last === null || first > last) return empty;
  const w = size(width), h = size(height);
  const edge = key => size(typeof inset === 'number' ? inset : inset?.[key] ?? 3);
  // Clamp each edge independently so even a zero/tiny viewport remains finite.
  const left = Math.min(w / 2, edge('left')), right = Math.min(w / 2, edge('right'));
  const top = Math.min(h / 2, edge('top')), bottom = Math.min(h / 2, edge('bottom'));
  const baseline = h - bottom, plotHeight = Math.max(0, baseline - top);
  const lookup = new Map();
  for (const row of Array.isArray(rows) ? rows : []) {
    const day = dayNumber(row?.date);
    if (day !== null && day >= first && day <= last) lookup.set(day, row.tokens);
  }
  const observed = [...lookup.entries()].filter(([, tokens]) => typeof tokens === 'number' && Number.isFinite(tokens) && tokens >= 0).sort((a, b) => a[0] - b[0]);
  if (!observed.length) return empty;
  const max = observed.reduce((n, [, tokens]) => Math.max(n, tokens), 0);
  const points = observed.map(([day, tokens]) => ({
    date: dateKey(day), tokens,
    x: left + (last === first ? .5 : (day - first) / (last - first)) * Math.max(0, w - left - right),
    y: baseline - (max > 0 ? tokens / max : 0) * plotHeight
  }));
  const n = points.length;
  // Conservative monotone Hermite tangents in calendar-day coordinates. Each
  // tangent is bounded by both adjacent secants, placing both Bezier controls
  // inside the interval's endpoint values. Thus no negative/peak overshoot is
  // possible, while adjacent curves share a tangent even across missing days.
  const secants = points.slice(1).map((point, i) => (point.y - points[i].y) / (observed[i + 1][0] - observed[i][0]));
  const slopes = points.map((_, i) => {
    if (n < 2) return 0;
    if (i === 0) return secants[0];
    if (i === n - 1) return secants[n - 2];
    const a = secants[i - 1], b = secants[i];
    return Math.sign(a) === Math.sign(b) ? Math.sign(a) * Math.min(Math.abs(a), Math.abs(b)) : 0;
  });
  const segments = [], gaps = [];
  let line = `M${pair(points[0].x, points[0].y)}`;
  for (let i = 1; i < n; i++) {
    const a = points[i - 1], b = points[i], days = observed[i][0] - observed[i - 1][0];
    const curve = `C${pair(a.x + (b.x - a.x) / 3, a.y + slopes[i - 1] * days / 3)} ${pair(b.x - (b.x - a.x) / 3, b.y - slopes[i] * days / 3)} ${pair(b.x, b.y)}`;
    const gap = days > 1;
    segments.push({ d: `M${pair(a.x, a.y)}${curve}`, gap });
    line += curve;
    if (gap) gaps.push({ start: dateKey(observed[i - 1][0] + 1), end: dateKey(observed[i][0] - 1), days: days - 1 });
  }
  const area = n > 1 ? `${line}L${pair(points[n - 1].x, baseline)}L${pair(points[0].x, baseline)}Z` : '';
  return { line, area, segments, points, gaps, missingDays: gaps.reduce((sum, gap) => sum + gap.days, 0), max };
}
