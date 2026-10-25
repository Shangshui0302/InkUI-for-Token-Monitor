import test from 'node:test';
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { planQuotaPages } from './quota-layout.mjs';

const account = (itemCount, extra = {}) => ({ itemCount, hasIdentity: true, hasNote: true, ...extra });
const EPSILON = 1e-6;
function assertCoverage(groups, plan, { width, height, font = 14 }) {
  assert.equal(plan.compact, false);
  const fragments = plan.pages.flat();
  assert.deepEqual([...new Set(fragments.map(part => part.groupIndex))], groups.map((_, index) => index));
  for (const page of plan.pages) {
    assert.ok(page.length > 0);
    let area = 0;
    for (const [index, part] of page.entries()) {
      assert.ok(part.x >= -EPSILON && part.y >= -EPSILON);
      assert.ok(part.x + part.width <= width + EPSILON);
      assert.ok(part.y + part.height <= height + EPSILON);
      assert.ok(part.width >= 180 - EPSILON);
      assert.equal(part.columns, part.count === 0 ? 1 : Math.min(part.count,
        Math.floor((part.width - 16 + 10 + EPSILON) / 155)));
      assert.ok(part.height >= 18 + part.headerHeight - EPSILON);
      if (part.count > 0) {
        assert.ok((part.width - 16 - (part.columns - 1) * 10) / part.columns >= 145 - EPSILON);
        const group = groups[part.groupIndex];
        const floor = Math.max(...Array.from({ length: part.count }, (_, index) =>
          group.metricHeights?.[part.start + index] ?? Math.ceil(font * 4.6 + 6)));
        assert.ok(part.metricHeight >= floor - EPSILON,
          `metric height ${part.metricHeight} is below ${floor}`);
        const rows = Math.ceil(part.count / part.columns);
        assert.ok(18 + part.headerHeight + 6 + rows * part.metricHeight + (rows - 1) * 6 <= part.height + EPSILON);
      }
      const group = groups[part.groupIndex];
      const headerMinimum = Math.max(Math.max(18, font * 1.3) * 1.2
        + (group.hasIdentity ? font * 0.72 * 1.2 + 3 : 0)
        + (group.hasNote ? font * 0.72 * 1.2 + 3 : 0),
      part.showAttachment && group.hasAttachment ? 24 : 0);
      assert.equal(part.headerHeight, headerMinimum);
      for (const other of page.slice(index + 1)) {
        assert.ok(part.x + part.width <= other.x + EPSILON
          || other.x + other.width <= part.x + EPSILON
          || part.y + part.height <= other.y + EPSILON
          || other.y + other.height <= part.y + EPSILON, 'cards overlap');
      }
      area += part.width * part.height;
    }
    assert.ok(area / (width * height) > 0.8, 'page should fill most of the usable viewport');
  }
  groups.forEach((group, groupIndex) => {
    const parts = fragments.filter(part => part.groupIndex === groupIndex);
    let end = 0;
    parts.forEach((part, index) => {
      assert.equal(part.start, end);
      assert.equal(part.continued, index > 0);
      assert.equal(part.showAttachment, index === 0);
      if (group.itemCount > 0) assert.ok(part.count > 0);
      if (part.count === 1) assert.equal(part.columns, 1);
      end += part.count;
    });
    assert.equal(end, group.itemCount);
    if (group.itemCount === 0) assert.equal(parts.length, 1);
  });
}

test('Poke3 has three distinct content-sized rectangles on one full page', () => {
  const groups = [account(1, { hasAttachment: true, metricHeights: [71] }),
    account(1, { hasIdentity: false, hasNote: false, metricHeights: [57] }), account(2, { metricHeights: [71, 71] })];
  const size = { width: 622, height: 279, font: 14 };
  const plan = planQuotaPages(groups, size);
  assert.equal(plan.pages.length, 1);
  assert.deepEqual(plan.pages[0].map(part => part.groupIndex), [0, 1, 2]);
  const areas = plan.pages[0].map(part => part.width * part.height);
  assert.equal(new Set(areas.map(Math.round)).size, 3);
  assert.ok(areas[2] > areas[0] && areas[0] > areas[1]);
  assert.ok(plan.pages[0][1].width < size.width);
  assert.ok(plan.pages[0][1].height < size.height, 'a balance can stack beside a taller account');
  assert.ok(areas.reduce((sum, area) => sum + area, 0) / (size.width * size.height) > 0.9);
  assertCoverage(groups, plan, size);
});

test('all-metadata Poke3 accounts also fit with nonuniform widths', () => {
  const groups = [account(1, { hasAttachment: true, metricHeights: [71] }),
    account(1, { metricHeights: [57] }), account(2)];
  const size = { width: 622, height: 279 };
  const plan = planQuotaPages(groups, size);
  assert.equal(plan.pages.length, 1);
  assert.equal(new Set(plan.pages[0].map(part => part.width)).size, 3);
  assertCoverage(groups, plan, size);
});

test('33 windows continue without dropping metrics or repeating the coupon', () => {
  const groups = [account(33, { hasAttachment: true,
    metricHeights: Array.from({ length: 33 }, (_, index) => index % 3 ? 57 : 71) }), account(3)];
  const size = { width: 662, height: 279 };
  const plan = planQuotaPages(groups, size);
  assert.ok(plan.pages.length > 1);
  assertCoverage(groups, plan, size);
});

test('zero-window accounts retain all metadata and coupon-only entries', () => {
  const groups = [account(0), account(1), account(0, { hasAttachment: true,
    hasIdentity: false, hasNote: false }), account(7)];
  const size = { width: 622, height: 279 };
  assertCoverage(groups, planQuotaPages(groups, size), size);
});

test('identity and status affect the area of otherwise identical records', () => {
  const groups = [account(1, { hasIdentity: false, hasNote: false }), account(1)];
  const size = { width: 622, height: 279 };
  const plan = planQuotaPages(groups, size);
  const areas = plan.pages[0].map(part => part.width * part.height);
  assert.ok(areas[1] > areas[0]);
  assertCoverage(groups, plan, size);
});

test('tiny and invalid viewports return a whole-plan fallback', () => {
  for (const size of [{ width: 128, height: 128 }, { width: 622, height: 40 },
    { width: 0, height: 279 }, { width: NaN, height: 279 }, { width: 622, height: Infinity }]) {
    const plan = planQuotaPages([account(0), account(5)], size);
    assert.equal(plan.compact, true);
    assert.deepEqual(plan.pages, []);
  }
  assert.deepEqual(planQuotaPages([], { width: 0, height: 0 }).pages, [[]]);
});

test('portrait, landscape and large viewports preserve minimum readable geometry', () => {
  for (const width of [240, 360, 453, 622, 662, 1072, 1920]) {
    for (const height of [180, 279, 453, 662, 1072]) {
      const groups = [0, 1, 2, 7, 33, 0].map((count, index) => account(count, {
        hasIdentity: index % 2 === 0, hasNote: index % 3 === 0, hasAttachment: index % 2 === 1,
        metricHeights: Array.from({ length: count }, (_, metric) => metric % 2 ? 57 : 71),
      }));
      const size = { width, height };
      assertCoverage(groups, planQuotaPages(groups, size), size);
    }
  }
});

test('bounded 128 accounts by 32 windows plans within an interactive budget', t => {
  const groups = Array.from({ length: 128 }, (_, index) => account(32, {
    hasAttachment: index % 2 === 0,
    metricHeights: Array.from({ length: 32 }, (_, metric) => metric % 2 ? 57 : 71),
  }));
  const size = { width: 622, height: 279 };
  const started = performance.now();
  const plan = planQuotaPages(groups, size);
  const elapsed = performance.now() - started;
  t.diagnostic(`128 × 32 windows: ${elapsed.toFixed(1)} ms, ${plan.pages.length} pages`);
  assert.ok(elapsed < 2000, `planning took ${elapsed.toFixed(1)} ms`);
  assertCoverage(groups, plan, size);
});
