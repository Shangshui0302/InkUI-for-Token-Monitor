import test from 'node:test';
import assert from 'node:assert/strict';
import { trendGeometry } from './charts.mjs';

const options = { start: '2026-10-01', end: '2026-10-10', width: 900, height: 100, inset: 0 };
const row = (day, tokens) => ({ date: `2026-10-${String(day).padStart(2, '0')}`, tokens });
const numbers = path => path.match(/-?\d+(?:\.\d+)?(?:e[+-]?\d+)?/gi)?.map(Number) || [];

// Evaluate the actual serialized cubic, rather than duplicating its algorithm.
function samples(d) {
  const [x0, y0, x1, y1, x2, y2, x3, y3] = numbers(d);
  return Array.from({ length: 101 }, (_, i) => {
    const t = i / 100, u = 1 - t;
    return { x: u ** 3 * x0 + 3 * u ** 2 * t * x1 + 3 * u * t ** 2 * x2 + t ** 3 * x3,
      y: u ** 3 * y0 + 3 * u ** 2 * t * y1 + 3 * u * t ** 2 * y2 + t ** 3 * y3 };
  });
}

test('missing dates connect continuously but are marked as unknown intervals', () => {
  const g = trendGeometry([row(2, 10), row(3, 20), row(8, 30)], options);
  assert.equal(g.segments.length, 2);
  assert.deepEqual(g.segments.map(s => s.gap), [false, true]);
  assert.deepEqual(g.gaps, [{ start: '2026-10-04', end: '2026-10-07', days: 4 }]);
  assert.equal(g.missingDays, 4);
  assert.equal((g.line.match(/M/g) || []).length, 1);
  assert.equal((g.line.match(/C/g) || []).length, 2);
  assert.equal(g.points[0].x, 100); // unknown leading day remains blank
  assert.equal(g.points.at(-1).x, 700); // no extrapolation through trailing days
  assert.ok(g.area.endsWith('L700,100L100,100Z'));
});

test('explicit zero is a real observation, null/invalid values are unknown', () => {
  const g = trendGeometry([row(1, 10), row(2, 0), row(3, null), row(4, 10), row(5, -1), row(6, '2'), row(7, NaN), row(8, Infinity)], options);
  assert.deepEqual(g.points.map(p => p.tokens), [10, 0, 10]);
  assert.equal(g.points[1].y, 100);
  assert.deepEqual(g.segments.map(s => s.gap), [false, true]);
  assert.deepEqual(g.gaps, [{ start: '2026-10-03', end: '2026-10-03', days: 1 }]);
});

test('sorting and last-row deduplication preserve the chosen observation', () => {
  const input = [row(4, 30), row(2, 3), row(1, 9), row(2, 12), row(1, null)];
  const snapshot = structuredClone(input);
  const g = trendGeometry(input, options);
  assert.deepEqual(g.points.map(p => [p.date, p.tokens]), [['2026-10-02', 12], ['2026-10-04', 30]]);
  assert.deepEqual(input, snapshot);
  assert.equal(g.max, 30);
});

test('serialized curves end at observed points and stay monotone within each interval', () => {
  const g = trendGeometry([row(1, 0), row(2, 100), row(3, 100), row(7, 1), row(8, 95), row(10, 0)], options);
  g.segments.forEach((segment, i) => {
    const a = g.points[i], b = g.points[i + 1], values = samples(segment.d);
    assert.deepEqual(values[0], { x: a.x, y: a.y });
    assert.deepEqual(values.at(-1), { x: b.x, y: b.y });
    for (let j = 0; j < values.length; j++) {
      assert.ok(values[j].y >= Math.min(a.y, b.y) - 1e-9 && values[j].y <= Math.max(a.y, b.y) + 1e-9);
      assert.ok(values[j].y >= -1e-9 && values[j].y <= 100 + 1e-9);
      if (j) {
        assert.ok(values[j].x >= values[j - 1].x);
        assert.ok((values[j].y - values[j - 1].y) * Math.sign(b.y - a.y) >= -1e-9);
      }
    }
  });
});

test('all-zero, singleton, and empty histories produce honest finite geometry', () => {
  const zero = trendGeometry([row(1, 0), row(2, 0)], options);
  assert.equal(zero.max, 0);
  assert.ok(zero.points.every(p => p.y === 100));
  const single = trendGeometry([row(4, 0)], options);
  assert.equal(single.line, 'M300,100');
  assert.equal(single.area, '');
  assert.deepEqual(single.segments, []);
  for (const rows of [[], null, [row(1, null)]]) {
    const g = trendGeometry(rows, options);
    assert.equal(g.line, '');
    assert.deepEqual(g.points, []);
    assert.equal(g.max, 0);
  }
});

test('tiny and degenerate viewports, large tokens, and a one-day range are finite', () => {
  for (const [width, height] of [[0, 0], [1, 1], [4, 2], [NaN, Infinity]]) {
    const g = trendGeometry([row(1, 0), row(2, Number.MAX_VALUE), row(3, Number.MAX_VALUE / 2)], { ...options, width, height, inset: 10 });
    assert.ok(numbers(g.line).every(Number.isFinite));
    assert.ok(g.points.every(p => p.x >= 0 && p.x <= (Number.isFinite(width) ? width : 0) && p.y >= 0 && p.y <= (Number.isFinite(height) ? height : 0)));
  }
  const g = trendGeometry([row(1, 1)], { ...options, end: options.start });
  assert.equal(g.points[0].x, 450);
  assert.ok(numbers(g.line).every(Number.isFinite));
});

test('strict calendar dates reject overflow and invalid bounds without throwing', () => {
  const g = trendGeometry([{ date: '2026-02-30', tokens: 20 }, { date: '2026-02-28', tokens: 1 }, { date: '2026-2-27', tokens: 3 }, null], { ...options, start: '2026-02-01', end: '2026-03-01' });
  assert.deepEqual(g.points.map(p => p.date), ['2026-02-28']);
  for (const bounds of [{ start: '2026-10-11' }, { end: 'invalid' }, { start: null }, { start: '2026-02-30' }]) assert.equal(trendGeometry([row(1, 1)], { ...options, ...bounds }).line, '');
  assert.equal(trendGeometry().line, '');
});

test('calendar-day spacing is constant over spring and autumn DST changes', () => {
  const previous = process.env.TZ;
  try {
    process.env.TZ = 'America/New_York';
    for (const [start, middle, end] of [['2026-03-07', '2026-03-08', '2026-03-09'], ['2026-10-31', '2026-11-01', '2026-11-02']]) {
      const g = trendGeometry([start, middle, end].map(date => ({ date, tokens: 1 })), { start, end, width: 200, height: 100, inset: 0 });
      assert.deepEqual(g.points.map(p => p.x), [0, 100, 200]);
      assert.deepEqual(g.segments.map(s => s.gap), [false, false]);
    }
  } finally {
    if (previous === undefined) delete process.env.TZ; else process.env.TZ = previous;
  }
});
