import assert from 'node:assert/strict';
import test from 'node:test';

import { formatEta, heroXpRequired, levelForecast } from './xp.js';

test('hero XP curve reproduces the published interpolation', () => {
  assert.equal(heroXpRequired(0), 3_208_120);
  assert.equal(heroXpRequired(10), 4_123_490);
  assert.equal(heroXpRequired(52), 13_950_774);
  assert.equal(heroXpRequired(100), 82_717_616);
});

test('forecast subtracts current progress only from the first level', () => {
  const rows = levelForecast(10, 20_236, 4_000_000, 3);
  assert.deepEqual(rows.map((row) => row.level), [11, 12, 13]);
  assert.equal(rows[0].remaining, 4_103_254);
  assert.equal(rows[1].remaining, heroXpRequired(11));
  assert.equal(rows[1].cumulativeXp, rows[0].remaining + heroXpRequired(11));
  assert.equal(rows[0].etaSeconds, (4_103_254 / 4_000_000) * 3600);
});

test('forecast has no ETA until the session has an XP rate', () => {
  assert.equal(levelForecast(10, 20_236, 0, 1)[0].etaSeconds, null);
});

test('ETA formatter keeps long forecasts compact', () => {
  assert.equal(formatEta(null), '—');
  assert.equal(formatEta(30), '<1m');
  assert.equal(formatEta(3_660), '1h 1m');
  assert.equal(formatEta(183_600), '2d 3h');
});
