// Software palettes reference common e-ink capabilities, not panel calibration.
export const themeKey = 'token-monitor-eink-theme-v1';
export const accentKey = 'token-monitor-eink-accent-v1';
export const ACCENTS = [
  { id: 'blue', label: '蓝' }, { id: 'teal', label: '青' },
  { id: 'green', label: '绿' }, { id: 'amber', label: '琥珀' },
  { id: 'rose', label: '玫瑰' }, { id: 'violet', label: '紫' }
];
export function normalizeAccent(value) { return ACCENTS.some(accent => accent.id === value) ? value : 'blue'; }
export function normalizeTheme(value) { return value === 'color' ? 'color' : 'bw'; }
export function heatLevel(value, maximum) {
  if (!Number.isFinite(value) || value <= 0 || !Number.isFinite(maximum) || maximum <= 0) return 0;
  return Math.max(1, Math.min(15, Math.ceil(value / maximum * 15)));
}
