const CARD_MIN_WIDTH = 180;
const CARD_INSET = 18; // 8px padding on both sides, plus the top border.
const METRIC_MIN_WIDTH = 145;
const METRIC_COLUMN_GAP = 10;
const METRIC_ROW_GAP = 6;
const GAP = 8;

/**
 * Tile a fixed viewport with content-weighted account rectangles.
 *
 * Compare ordered weighted rows with a two-column, vertically stacked slicing
 * layout. Row partitions use dynamic programming; the alternative direction
 * tries a bounded set of content and metric-width thresholds. A page is
 * repacked as accounts are added. Only
 * when a complete account cannot fit do we split its windows, in source order.
 * Work per packing attempt is bounded by account count and viewport width.
 */
export function planQuotaPages(groups, { width, height, font = 14 } = {}) {
  const result = { compact: false, gap: GAP, pages: [] };
  if (!Array.isArray(groups) || groups.length === 0) return { ...result, pages: [[]] };
  const compact = () => ({ ...result, compact: true, pages: [] });
  if (![width, height, font].every(value => Number.isFinite(value) && value > 0)
    || width < CARD_MIN_WIDTH) return compact();

  const defaultMetricHeight = Math.ceil(font * 4.6 + 6);
  const accounts = groups.map(group => ({
    itemCount: Number.isSafeInteger(group?.itemCount) && group.itemCount > 0 ? group.itemCount : 0,
    hasIdentity: Boolean(group?.hasIdentity),
    hasNote: Boolean(group?.hasNote),
    hasAttachment: Boolean(group?.hasAttachment),
    metricHeights: Array.isArray(group?.metricHeights) ? group.metricHeights : [],
  }));
  const textLine = font * 0.72 * 1.2 + 3;
  const makeFragment = (groupIndex, start, count) => {
    const group = accounts[groupIndex];
    const headerHeight = Math.max(
      Math.max(18, font * 1.3) * 1.2
        + (group.hasIdentity ? textLine : 0) + (group.hasNote ? textLine : 0),
      start === 0 && group.hasAttachment ? 24 : 0,
    );
    let minimumMetricHeight = 0;
    for (let index = start; index < start + count; index += 1) {
      const supplied = group.metricHeights[index];
      minimumMetricHeight = Math.max(minimumMetricHeight,
        Number.isFinite(supplied) && supplied > 0 ? supplied : defaultMetricHeight);
    }
    // The header consumes real area too, including records with no windows.
    const weight = CARD_INSET + headerHeight
      + (count > 0 ? 6 + count * (minimumMetricHeight + METRIC_ROW_GAP) : 0);
    return { groupIndex, start, count, showAttachment: start === 0,
      continued: start > 0, headerHeight, minimumMetricHeight, weight };
  };
  const geometry = (fragment, cardWidth) => {
    const columns = fragment.count === 0 ? 1 : Math.max(1, Math.min(fragment.count,
      Math.floor((cardWidth - 16 + METRIC_COLUMN_GAP) / (METRIC_MIN_WIDTH + METRIC_COLUMN_GAP))));
    const rows = Math.ceil(fragment.count / columns);
    const minimumHeight = CARD_INSET + fragment.headerHeight + (rows > 0
      ? 6 + rows * fragment.minimumMetricHeight + (rows - 1) * METRIC_ROW_GAP : 0);
    return { columns, rows, minimumHeight };
  };
  const maxCardsPerRow = Math.max(1, Math.floor((width + GAP) / (CARD_MIN_WIDTH + GAP)));

  const pack = fragments => {
    const length = fragments.length;
    const costs = Array(length + 1).fill(Infinity);
    const previous = Array(length + 1);
    costs[0] = 0;
    for (let end = 1; end <= length; end += 1) {
      for (let start = Math.max(0, end - maxCardsPerRow); start < end; start += 1) {
        if (!Number.isFinite(costs[start])) continue;
        const cards = fragments.slice(start, end);
        const spareWidth = width - GAP * (cards.length - 1) - CARD_MIN_WIDTH * cards.length;
        // Cubic weights make additional windows visibly larger, while reserving
        // a readable minimum for every account and its metadata.
        const weightTotal = cards.reduce((sum, card) => sum + card.weight ** 3, 0);
        const widths = cards.map(card => CARD_MIN_WIDTH + spareWidth * card.weight ** 3 / weightTotal);
        const metrics = cards.map((card, index) => geometry(card, widths[index]));
        const minimumHeight = Math.max(...metrics.map(metric => metric.minimumHeight));
        const cost = costs[start] + (start > 0 ? GAP : 0) + minimumHeight;
        if (cost <= height + 1e-7 && cost < costs[end] - 1e-7) {
          costs[end] = cost;
          previous[end] = { start, cards, widths, metrics, minimumHeight,
            weight: cards.reduce((sum, card) => sum + card.weight, 0) };
        }
      }
    }
    let best = null;
    let bestScore = Infinity;
    const consider = page => {
      // Prefer cards whose height is close to their actual content. In
      // particular, a balance-only card should not inherit a tall neighbour's
      // unused vertical area when a stacked alternative is possible.
      const totalArea = page.reduce((sum, card) => sum + card.width * card.height, 0);
      const totalWeight = fragments.reduce((sum, fragment) => sum + fragment.weight, 0);
      const score = page.reduce((sum, card, index) => {
        const fragment = fragments[index];
        const minimum = geometry(fragment, card.width).minimumHeight;
        const areaShare = card.width * card.height / totalArea;
        const contentShare = fragment.weight / totalWeight;
        return sum + Math.log(card.height / minimum) ** 2
          + 0.15 * Math.log(card.width / CARD_MIN_WIDTH) ** 2
          + 2 * Math.log(areaShare / contentShare) ** 2;
      }, 0);
      if (score < bestScore - 1e-8) { best = page; bestScore = score; }
    };
    if (previous[length]) {
      const rows = [];
      for (let end = length; end > 0;) {
        const row = previous[end];
        rows.unshift(row);
        end = row.start;
      }
      const spareHeight = Math.max(0, height - costs[length]);
      const weightTotal = rows.reduce((sum, row) => sum + row.weight, 0);
      const page = [];
      let y = 0;
      rows.forEach((row, rowIndex) => {
        const rowHeight = rowIndex === rows.length - 1 ? height - y
          : row.minimumHeight + spareHeight * row.weight / weightTotal;
        let x = 0;
        row.cards.forEach((fragment, index) => {
          const cardWidth = index === row.cards.length - 1 ? width - x : row.widths[index];
          const { columns, rows: metricRows } = row.metrics[index];
          const metricHeight = metricRows > 0
            ? (rowHeight - CARD_INSET - fragment.headerHeight - 6
              - (metricRows - 1) * METRIC_ROW_GAP) / metricRows : 0;
          const { minimumMetricHeight, weight, ...fields } = fragment;
          page.push({ ...fields, x, y, width: cardWidth, height: rowHeight, columns, metricHeight });
          x += cardWidth + GAP;
        });
        y += rowHeight + GAP;
      });
      consider(page);
    }

    // A second, independent slicing direction: each of two content-sized
    // columns stacks its accounts. Candidate widths include metric column
    // thresholds, so a two-window card can trade width for a shorter height.
    // Limit split candidates for very large metadata-only inputs.
    const splitPoints = length <= 16 ? Array.from({ length: length - 1 }, (_, index) => index + 1)
      : [...new Set(Array.from({ length: 15 }, (_, index) => Math.round(length * (index + 1) / 16)))];
    for (const split of splitPoints) {
      const left = fragments.slice(0, split);
      const right = fragments.slice(split);
      const totalWidth = width - GAP;
      if (totalWidth < CARD_MIN_WIDTH * 2) break;
      const leftWeight = left.reduce((sum, card) => sum + card.weight, 0);
      const rightWeight = right.reduce((sum, card) => sum + card.weight, 0);
      const widths = new Set([CARD_MIN_WIDTH, totalWidth - CARD_MIN_WIDTH, totalWidth / 2,
        totalWidth * leftWeight / (leftWeight + rightWeight)]);
      for (let columns = 2; columns <= maxCardsPerRow; columns += 1) {
        const threshold = 16 + columns * METRIC_MIN_WIDTH + (columns - 1) * METRIC_COLUMN_GAP;
        widths.add(threshold);
        widths.add(totalWidth - threshold);
      }
      for (const leftWidth of widths) {
        if (leftWidth < CARD_MIN_WIDTH || totalWidth - leftWidth < CARD_MIN_WIDTH) continue;
        const candidate = [];
        let x = 0;
        let fits = true;
        for (const [cards, cardWidth] of [[left, leftWidth], [right, totalWidth - leftWidth]]) {
          const metrics = cards.map(card => geometry(card, cardWidth));
          const minimum = metrics.reduce((sum, metric) => sum + metric.minimumHeight, 0);
          const available = height - GAP * (cards.length - 1);
          if (minimum > available + 1e-7) { fits = false; break; }
          let y = 0;
          cards.forEach((fragment, index) => {
            const metric = metrics[index];
            const cardHeight = index === cards.length - 1 ? height - y
              : metric.minimumHeight * available / minimum;
            const metricHeight = metric.rows > 0
              ? (cardHeight - CARD_INSET - fragment.headerHeight - 6
                - (metric.rows - 1) * METRIC_ROW_GAP) / metric.rows : 0;
            const { minimumMetricHeight, weight, ...fields } = fragment;
            candidate.push({ ...fields, x, y, width: cardWidth, height: cardHeight,
              columns: metric.columns, metricHeight });
            y += cardHeight + GAP;
          });
          x += cardWidth + GAP;
        }
        if (fits) consider(candidate);
      }
    }
    return best;
  };

  // Fall back as a whole: returning a partial plan would hide later records.
  if (accounts.some((group, index) => !pack([makeFragment(index, 0, group.itemCount > 0 ? 1 : 0)]))) {
    return compact();
  }
  let pending = [];
  let packed = null;
  const finishPage = () => {
    result.pages.push(packed);
    pending = [];
    packed = null;
  };
  for (let groupIndex = 0; groupIndex < accounts.length; groupIndex += 1) {
    const group = accounts[groupIndex];
    let start = 0;
    do {
      const remaining = group.itemCount - start;
      const attempt = count => pack([...pending, makeFragment(groupIndex, start, count)]);
      let count = remaining;
      let candidate = attempt(count);
      // Width changes can cross a metric-column threshold, so feasibility is
      // not monotonic in count. Check whole fragments from largest to smallest.
      while (!candidate && count > 1) {
        count -= 1;
        candidate = attempt(count);
      }
      if (!candidate) {
        if (!pending.length) return compact();
        finishPage();
        continue;
      }
      pending.push(makeFragment(groupIndex, start, count));
      packed = candidate;
      start += count;
      if (start < group.itemCount) finishPage();
      else break;
    } while (true);
  }
  if (pending.length) finishPage();
  return result;
}
