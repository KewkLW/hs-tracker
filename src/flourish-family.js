export const FLOURISH_FAMILIES = Object.freeze([
  Object.freeze({ id: 'loot', label: 'Loot drops', description: 'Ordinary rarity and custom-filter announcements' }),
  Object.freeze({ id: 'high_roll', label: 'High rolls', description: 'Items whose variable stats clear the roll threshold' }),
  Object.freeze({ id: 'stat', label: 'Custom stat alerts', description: 'Stat rules, including an item that is also a high roll' }),
  Object.freeze({ id: 'zone', label: 'Zone alerts', description: 'Satanic rotations and Colossal Chest zones' }),
  Object.freeze({ id: 'twitch', label: 'Twitch alerts', description: 'Follows, subs, raids and other Twitch events' }),
]);

const FAMILY_IDS = new Set(FLOURISH_FAMILIES.map(({ id }) => id));
export const FLOURISH_FAMILY_LABELS = Object.freeze(
  Object.fromEntries(FLOURISH_FAMILIES.map(({ id, label }) => [id, label])),
);

export function normaliseFlourishFamily(value, fallback = 'loot') {
  const family = String(value ?? '');
  return FAMILY_IDS.has(family) ? family : (FAMILY_IDS.has(fallback) ? fallback : 'loot');
}

// Routing order is intentional. A custom-stat match owns a combined
// stat+high-roll alert, and every Colossal Chest announcement uses the zone
// location even if an older payload omitted kind: 'zone'.
export function flourishFamily(entry) {
  if (entry?.kind === 'zone' || entry?.colossal_chest) return 'zone';
  if (entry?.kind === 'twitch') return 'twitch';
  if ((entry?.stat_matches?.length ?? 0) > 0) return 'stat';
  if (entry?.high_roll) return 'high_roll';
  return 'loot';
}

// Older backends emitted a bare boolean. Keep it meaningful as the original
// loot placement while accepting the family-aware object from current builds.
export function normalisePlacementEvent(payload) {
  if (typeof payload === 'boolean') return { placing: payload, family: 'loot', legacy: true };
  const value = payload && typeof payload === 'object' ? payload : {};
  return {
    placing: Boolean(value.placing),
    family: normaliseFlourishFamily(value.family),
    legacy: false,
  };
}
