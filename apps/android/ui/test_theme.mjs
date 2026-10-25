import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { ACCENTS, heatLevel, normalizeAccent, normalizeTheme } from './theme.mjs';

const css = readFileSync(new URL('./themes.css', import.meta.url), 'utf8');
const rules = [...css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^}]+)\}/g)].map(match => ({
  selectors: match[1].trim().split(',').map(selector => selector.trim()),
  values: Object.fromEntries([...match[2].matchAll(/--([\w-]+):\s*([^;]+);/g)].map(m => [m[1], m[2].trim()]))
}));
// Resolve matching root rules in CSS specificity/source order, including inherited
// defaults and variable aliases used by the semantic and chart colors.
function paletteFor(theme, accent) {
  const attributes = { 'data-theme': theme, 'data-accent': accent };
  const matching = rules.flatMap((rule, order) => rule.selectors.filter(selector => {
    if (!/^:root(?:\[[^\]]+\])*$/.test(selector)) return false;
    return [...selector.matchAll(/\[([^=]+)="([^"]+)"\]/g)].every(([, key, value]) => attributes[key] === value);
  }).map(selector => ({ ...rule, order, specificity: (selector.match(/\[/g) || []).length })));
  matching.sort((a, b) => a.specificity - b.specificity || a.order - b.order);
  const values = Object.assign({}, ...matching.map(rule => rule.values));
  const resolve = (value, seen = new Set()) => value.replace(/var\(--([\w-]+)\)/g, (_, key) => {
    assert.ok(!seen.has(key), 'cyclic variable: ' + key);
    assert.ok(values[key], 'undefined variable: ' + key);
    return resolve(values[key], new Set([...seen, key]));
  });
  return Object.fromEntries(Object.entries(values).map(([key, value]) => [key, resolve(value)]));
}
const rgb = value => value.slice(1).match(/../g).map(x => parseInt(x, 16));
const luminance = value => rgb(value).map(x => x / 255).map(x => x <= .04045 ? x / 12.92 : ((x + .055) / 1.055) ** 2.4).reduce((s, x, i) => s + x * [.2126, .7152, .0722][i], 0);
const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + .05) / (Math.min(luminance(a), luminance(b)) + .05);
const accentIds = ['blue', 'teal', 'green', 'amber', 'rose', 'violet'];
const palettes = [['bw', paletteFor('bw')], ...accentIds.map(accent => [accent, paletteFor('color', accent)])];

test('unknown or missing stored theme safely selects monochrome', () => {
  for (const value of [null, undefined, '', 'eink', 'dark', {}, true]) assert.equal(normalizeTheme(value), 'bw');
  assert.equal(normalizeTheme('color'), 'color');
  assert.equal(normalizeTheme('bw'), 'bw');
});

test('six known accents are retained and unknown stored accents select blue', () => {
  assert.deepEqual(ACCENTS.map(accent => accent.id), accentIds);
  for (const value of [null, undefined, '', 'red', 'bw', {}, true]) assert.equal(normalizeAccent(value), 'blue');
  for (const accent of accentIds) assert.equal(normalizeAccent(accent), accent);
  assert.deepEqual(paletteFor('color'), paletteFor('color', 'blue'));
});

test('heat intensity keeps missing and zero empty, and positive samples visible', () => {
  for (const value of [null, undefined, NaN, Infinity, -1, 0]) assert.equal(heatLevel(value, 100), 0);
  for (const maximum of [null, NaN, Infinity, 0, -1]) assert.equal(heatLevel(10, maximum), 0);
  assert.equal(heatLevel(.001, 100), 1);
  assert.equal(heatLevel(50, 100), 8);
  assert.equal(heatLevel(100, 100), 15);
  assert.equal(heatLevel(101, 100), 15);
});

test('all heat palettes have 16 distinct monotonically darker quantized steps', () => {
  for (const [name, palette] of palettes) {
    const ramp = Array.from({ length: 16 }, (_, i) => palette['heat-' + i]);
    assert.equal(new Set(ramp).size, 16, name);
    for (const color of Object.values(palette)) {
      assert.match(color, /^#[\da-f]{6}$/, name);
      assert.ok(rgb(color).every(channel => channel % 17 === 0), name + ': ' + color);
      if (name === 'bw') assert.equal(new Set(rgb(color)).size, 1);
    }
    for (let i = 1; i < ramp.length; i++) assert.ok(luminance(ramp[i]) < luminance(ramp[i - 1]), name + ': ' + i);
  }
  // Include the individual choice dots, which do not appear in root palettes.
  for (const [, color] of css.matchAll(/:\s*(#[\da-f]{6})/g)) assert.ok(rgb(color).every(channel => channel % 17 === 0), color);
});

test('primary, secondary and semantic text retain at least 4.5:1 contrast', () => {
  for (const [name, palette] of palettes) for (const foreground of ['ink', 'muted', 'accent-ink', 'ok', 'warn', 'danger']) {
    const background = foreground === 'accent-ink' ? palette['accent-soft'] : palette.paper;
    assert.ok(contrast(background, palette[foreground]) >= 4.5, name + ': ' + foreground);
  }
});

test('color themes keep surfaces and ordinary text neutral, with one emphasis hue', () => {
  for (const accent of accentIds) {
    const palette = paletteFor('color', accent);
    assert.equal(palette.paper, '#ffffff');
    assert.equal(palette.ink, '#111111');
    for (const key of ['muted', 'line', 'soft-line', 'panel']) assert.equal(new Set(rgb(palette[key])).size, 1, key);
    assert.equal(palette['chart-line'], palette.accent);
    assert.equal(palette['accent-soft'], palette.accent);
    assert.equal(palette.ok, palette['accent-text']);
    assert.ok(luminance(palette.accent) >= .2, accent + ': bright emphasis');
    assert.ok(contrast(palette['accent-ink'], palette.accent) >= 4.5, accent + ': text on bright fill');
    assert.ok(contrast(palette['accent-text'], palette.paper) >= 4.5, accent + ': text on paper');
    // Remembering a color preference must not leak its colors into monochrome.
    assert.deepEqual(paletteFor('bw', accent), paletteFor('bw'));
  }
  assert.equal(new Set(accentIds.map(accent => paletteFor('color', accent).accent)).size, 6);
});
