import test from 'node:test';
import assert from 'node:assert/strict';

import { flourishFamily, normalisePlacementEvent } from './flourish-family.js';

test('alert payloads route to their independent placement families', () => {
  assert.equal(flourishFamily({ rarity: 'Satanic' }), 'loot');
  assert.equal(flourishFamily({ high_roll: true }), 'high_roll');
  assert.equal(flourishFamily({ stat_matches: [{ stat_id: 70 }] }), 'stat');
  assert.equal(flourishFamily({ high_roll: true, stat_matches: [{ stat_id: 70 }] }), 'stat');
  assert.equal(flourishFamily({ kind: 'zone' }), 'zone');
  assert.equal(flourishFamily({ colossal_chest: true }), 'zone');
  assert.equal(flourishFamily({ kind: 'twitch', event: 'raid' }), 'twitch');
});

test('placement events accept current objects and legacy booleans', () => {
  assert.deepEqual(normalisePlacementEvent({ placing: true, family: 'stat' }), {
    placing: true,
    family: 'stat',
    legacy: false,
  });
  assert.deepEqual(normalisePlacementEvent(true), { placing: true, family: 'loot', legacy: true });
  assert.deepEqual(normalisePlacementEvent(false), { placing: false, family: 'loot', legacy: true });
  assert.equal(normalisePlacementEvent({ placing: true, family: 'unknown' }).family, 'loot');
});
