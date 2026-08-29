import assert from 'node:assert/strict';
import test from 'node:test';
import {
  DEFAULT_FX_PROFILE,
  FX_PRESET_LIMIT,
  STARTER_FX_PRESETS,
  applyPalette,
  defaultFxProfile,
  fxBundle,
  makeFxPreset,
  normaliseFxPresets,
  normaliseFxProfile,
  parseFxBundle,
} from './fx-presets.js';

test('classic alerts keep the optional loot beam off', () => {
  const classic = STARTER_FX_PRESETS.find((preset) => preset.id === 'starter-hero-siege');
  const frost = STARTER_FX_PRESETS.find((preset) => preset.id === 'starter-frost-orb');
  assert.equal(DEFAULT_FX_PROFILE.beam_enabled, false);
  assert.equal(classic.fx.beam_enabled, false);
  assert.equal(frost.fx.beam_enabled, true);
});

test('FX profiles clamp imported values and keep quality bands ordered', () => {
  const fx = normaliseFxProfile({
    duration_s: 200,
    scale: -2,
    quality_epic: 96,
    quality_near: 80,
    quality_perfect: 90,
    colors: { high_roll: 'not a colour', stat: '#AABBCC' },
  });
  assert.equal(fx.duration_s, 12);
  assert.equal(fx.scale, 0.5);
  assert.deepEqual([fx.quality_epic, fx.quality_near, fx.quality_perfect], [96, 96, 96]);
  assert.equal(fx.colors.high_roll, DEFAULT_FX_PROFILE.colors.high_roll);
  assert.equal(fx.colors.stat, '#aabbcc');
});

test('choosing a palette changes its colors without sharing mutable defaults', () => {
  const frost = applyPalette(defaultFxProfile(), 'frost');
  frost.colors.stat = '#000000';
  assert.notEqual(defaultFxProfile().colors.stat, '#000000');
  assert.equal(frost.palette, 'frost');
});

test('preset bundles round trip and reject unrelated JSON', () => {
  const made = makeFxPreset('Ice', applyPalette(defaultFxProfile(), 'frost'), 'ice');
  const parsed = parseFxBundle(JSON.stringify(fxBundle([made], made.fx)));
  assert.equal(parsed.presets[0].id, 'ice');
  assert.equal(parsed.presets[0].fx.palette, 'frost');
  assert.throws(() => parseFxBundle('{"app":"elsewhere"}'), /not an HS Tracker/);
});

test('preset lists reject duplicate ids and are bounded', () => {
  const many = Array.from({ length: FX_PRESET_LIMIT + 10 }, (_, index) =>
    makeFxPreset(`Preset ${index}`, defaultFxProfile(), `p-${Math.min(index, FX_PRESET_LIMIT)}`),
  );
  const clean = normaliseFxPresets(many);
  assert.equal(clean.length, FX_PRESET_LIMIT);
  assert.equal(new Set(clean.map((preset) => preset.id)).size, clean.length);
});
