// Hero-level XP estimates published by HS Helper for Season 9. The source
// describes the curve as interpolation between known values, so keep the
// anchors instead of pretending these are exact Season 10 game constants.
// Season 10's client still calculates the value internally, but its YYC build
// compiles that function into the executable rather than exposing a table.
const HERO_XP_ANCHORS = [
  [0, 3_208_119.5],
  [3, 3_412_559],
  // Observed in the Season 10 client at HLv 10 (20,236 / 4,123,490).
  [10, 4_123_490],
  [21, 5_265_277],
  [36, 8_196_982],
  [38, 8_734_805],
  [52, 13_950_774],
  [72, 28_654_905],
  [149, 177_327_360.25],
];

export function heroXpRequired(level) {
  if (!Number.isFinite(level) || level < 0) return null;
  const whole = Math.floor(level);
  for (let i = 1; i < HERO_XP_ANCHORS.length; i += 1) {
    const [rightLevel, rightXp] = HERO_XP_ANCHORS[i];
    if (whole > rightLevel) continue;
    const [leftLevel, leftXp] = HERO_XP_ANCHORS[i - 1];
    const fraction = (whole - leftLevel) / (rightLevel - leftLevel);
    return Math.round(leftXp + (rightXp - leftXp) * fraction);
  }

  // The published curve is linear from HLv 72 onward. Continuing that final
  // segment is more useful than making the panel disappear above HLv 149, and
  // is labelled as an estimate in the interface with the rest of the curve.
  const [leftLevel, leftXp] = HERO_XP_ANCHORS.at(-2);
  const [rightLevel, rightXp] = HERO_XP_ANCHORS.at(-1);
  const perLevel = (rightXp - leftXp) / (rightLevel - leftLevel);
  return Math.round(rightXp + (whole - rightLevel) * perLevel);
}

export function levelForecast(heroLevel, xpInLevel, xpPerHour, count = 10) {
  if (!Number.isFinite(heroLevel) || heroLevel < 0 || count < 1) return [];
  const current = Math.floor(heroLevel);
  const banked = Math.max(0, Number(xpInLevel) || 0);
  const rate = Math.max(0, Number(xpPerHour) || 0);
  const rows = [];
  let cumulativeXp = 0;

  for (let offset = 0; offset < count; offset += 1) {
    const fromLevel = current + offset;
    const required = heroXpRequired(fromLevel);
    if (required == null) break;
    const levelXp = offset === 0 ? Math.max(0, required - banked) : required;
    cumulativeXp += levelXp;
    rows.push({
      level: fromLevel + 1,
      required,
      remaining: levelXp,
      cumulativeXp,
      etaSeconds: rate > 0 ? (cumulativeXp / rate) * 3600 : null,
    });
  }
  return rows;
}

export function formatEta(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return '—';
  if (seconds < 60) return '<1m';
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const mins = minutes % 60;
  if (hours < 24) return mins ? `${hours}h ${mins}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return remHours ? `${days}d ${remHours}h` : `${days}d`;
}
