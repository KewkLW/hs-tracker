import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { ITEM_STATS, statLabel } from './item-stats.js';

test('the S10 stat picker catalog stays sorted, unique and exhaustive', () => {
  const ids = ITEM_STATS.map(({ id }) => id);
  assert.equal(new Set(ids).size, ids.length);
  assert.deepEqual(ids, [...ids].sort((left, right) => left - right));
  assert.ok(ids.length >= 322, 'the audited S10 catalog unexpectedly lost entries');

  for (const descriptor of [10, 11, 12, 13, 14, 15, 431, 432]) {
    assert.ok(!ids.includes(descriptor), `metadata/descriptor id ${descriptor} entered the picker`);
  }
  for (const actual of [20, 70, 71, 154, 202, 437, 448]) {
    assert.ok(ids.includes(actual), `actual packet stat id ${actual} is missing`);
  }

  const generated = readFileSync(new URL('../src-tauri/src/item_rolls.rs', import.meta.url), 'utf8');
  const rollIds = [...generated.matchAll(/StatRange \{ id: (\d+)/g)].map((match) => Number(match[1]));
  for (const id of new Set(rollIds)) {
    assert.ok(ids.includes(id), `variable roll stat id ${id} is missing from the picker`);
  }
});

test('flat and percent projectile speed remain distinguishable', () => {
  assert.equal(statLabel(70), 'Projectile Speed (flat)');
  assert.equal(statLabel('71'), 'Projectile Speed (%)');
  assert.equal(statLabel(154), 'Defense');
  assert.equal(statLabel(9999), 'Stat #9999');
});
