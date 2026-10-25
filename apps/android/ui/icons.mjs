const paths = {
  'chevron-down': ['m6 9 6 6 6-6'], 'chevron-up': ['m6 15 6-6 6 6'],
  check: ['m5 12 4 4L19 6'], grid: ['M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z'],
  ticket: ['M3 5h18v5a2 2 0 0 0 0 4v5H3v-5a2 2 0 0 0 0-4zM15 5v3m0 3v2m0 3v3'],
  user: ['M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0M4 21v-2a8 8 0 0 1 16 0v2'],
  clock: ['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M12 7v5l3 2'],
  'arrow-left': ['M20 12H4m6-6-6 6 6 6'], 'arrow-right': ['M4 12h16m-6-6 6 6-6 6'],
  refresh: ['M20 7v5h-5M4 17v-5h5M5.3 7a8 8 0 0 1 13-1L20 8M4 16l1.7 2a8 8 0 0 0 13-1'],
  settings: ['M4 6h16M4 12h16M4 18h16M8 3v6M16 9v6M10 15v6'],
  expand: ['M9 3H3v6M15 3h6v6M3 15v6h6M21 15v6h-6'],
  collapse: ['M3 9h6V3M21 9h-6V3M9 21v-6H3M15 21v-6h6'],
  wifi: ['M2 8a16 16 0 0 1 20 0M5 12a11 11 0 0 1 14 0M8.5 16a5.5 5.5 0 0 1 7 0M12 20h.01'],
  ethernet: ['M7 3h10v7H7zM12 10v4M3 21v-7h18v7M2 18h4M10 18h4M18 18h4'],
  cellular: ['M4 20v-3M9 20v-7M14 20V9M19 20V4'],
  offline: ['M2 8a16 16 0 0 1 20 0M5 12a11 11 0 0 1 14 0M8.5 16a5.5 5.5 0 0 1 7 0M12 20h.01M3 3l18 18'],
  other: ['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M3 12h18M12 3a16 16 0 0 1 0 18 16 16 0 0 1 0-18'],
  unknown: ['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0M9 8a3 3 0 1 1 4 3c-1 0-1 1-1 2M12 17h.01'],
  link: ['m10 13 4-4M8 16l-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0M13 17a4 4 0 0 0 6 0l4-4a4 4 0 0 0-6-6l-1 1'],
  'link-off': ['M3 3l18 18M8 16l-1 1a4 4 0 0 1-6-6l2-2M13 17a4 4 0 0 0 6 0l4-4a4 4 0 0 0-6-6l-1 1'],
  battery: ['M3 7h15v10H3zM21 10v4'], bolt: ['m13 2-7 11h6l-1 9 7-12h-6z']
};
export function icon(name) {
  const ns = 'http://www.w3.org/2000/svg', svg = document.createElementNS(ns, 'svg');
  for (const [key, value] of Object.entries({ viewBox:'0 0 24 24', class:'icon', 'aria-hidden':'true', focusable:'false', fill:'none', stroke:'currentColor', 'stroke-width':'1.7', 'stroke-linecap':'round', 'stroke-linejoin':'round' })) svg.setAttribute(key, value);
  for (const d of paths[name] || paths.unknown) { const p = document.createElementNS(ns, 'path'); p.setAttribute('d', d); svg.append(p); }
  return svg;
}
