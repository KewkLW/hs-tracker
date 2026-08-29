// The flourish renderer is deliberately data-driven. Keeping the shape and
// validation here means the settings page, imported packs and the renderer all
// agree on what an FX preset is, even when a newer preset is opened by an older
// build of the tracker.

export const FX_PROFILE_VERSION = 1;
export const FX_PRESET_LIMIT = 48;

export const PALETTES = {
  'hero-siege': {
    label: 'Hero Siege',
    colors: {
      ordinary: '#f0e0b0',
      high_roll: '#b56cff',
      near_perfect: '#ffd36a',
      perfect: '#fff5c2',
      stat: '#35d9ff',
      combined: '#75b6ff',
    },
  },
  frost: {
    label: 'Frost',
    colors: {
      ordinary: '#dff8ff',
      high_roll: '#79cfff',
      near_perfect: '#8beaff',
      perfect: '#ffffff',
      stat: '#42e5ff',
      combined: '#9fb6ff',
    },
  },
  demonic: {
    label: 'Demonic',
    colors: {
      ordinary: '#f3c0a4',
      high_roll: '#ff4b45',
      near_perfect: '#ff8a3d',
      perfect: '#ffe2a8',
      stat: '#d553ff',
      combined: '#ff397d',
    },
  },
  celestial: {
    label: 'Celestial',
    colors: {
      ordinary: '#f8efd5',
      high_roll: '#b8a1ff',
      near_perfect: '#ffe98a',
      perfect: '#ffffff',
      stat: '#78e8d4',
      combined: '#d8c2ff',
    },
  },
  neon: {
    label: 'Clean neon',
    colors: {
      ordinary: '#e8f7ff',
      high_roll: '#c45cff',
      near_perfect: '#ffef5a',
      perfect: '#ffffff',
      stat: '#00f0ff',
      combined: '#568cff',
    },
  },
};

const DEFAULT_COLORS = PALETTES['hero-siege'].colors;

export const DEFAULT_FX_PROFILE = Object.freeze({
  version: FX_PROFILE_VERSION,
  layout: 'cinematic',
  palette: 'hero-siege',
  entrance: 'rise',
  duration_s: 6,
  scale: 1,
  shade: 0.55,
  font_scale: 1,
  show_heading: true,
  show_item_name: true,
  show_tier: true,
  show_stat: true,
  glow_enabled: true,
  glow_intensity: 1,
  // The original alert has no vertical loot pillar. Keep it opt-in so existing
  // users do not suddenly get a large beam behind otherwise-classic alerts.
  beam_enabled: false,
  shockwave_enabled: false,
  screen_flash: 0,
  particles_enabled: true,
  particle_density: 45,
  particle_size: 1,
  particle_speed: 1,
  particle_trails: false,
  quality_escalation: true,
  quality_epic: 85,
  quality_near: 95,
  quality_perfect: 100,
  stat_fx_enabled: true,
  projectile_trails: true,
  vitality_pulse: true,
  crushing_shockwave: true,
  socket_orbit: true,
  edge_position: 'top',
  edge_inset: 24,
  reduce_motion: false,
  colors: DEFAULT_COLORS,
});

const ENUMS = {
  layout: ['cinematic', 'compact', 'edge'],
  palette: Object.keys(PALETTES),
  entrance: ['rise', 'slam', 'rift', 'fade'],
  edge_position: ['top', 'bottom', 'left', 'right'],
};

const NUMBERS = {
  duration_s: [2, 12],
  scale: [0.5, 2],
  shade: [0, 0.9],
  font_scale: [0.7, 1.6],
  glow_intensity: [0, 1.5],
  screen_flash: [0, 1],
  particle_density: [0, 100],
  particle_size: [0.5, 2],
  particle_speed: [0.5, 2],
  quality_epic: [0, 100],
  quality_near: [0, 100],
  quality_perfect: [0, 100],
  edge_inset: [0, 200],
};

const BOOLEANS = [
  'show_heading',
  'show_item_name',
  'show_tier',
  'show_stat',
  'glow_enabled',
  'beam_enabled',
  'shockwave_enabled',
  'particles_enabled',
  'particle_trails',
  'quality_escalation',
  'stat_fx_enabled',
  'projectile_trails',
  'vitality_pulse',
  'crushing_shockwave',
  'socket_orbit',
  'reduce_motion',
];

const COLOR_KEYS = Object.keys(DEFAULT_COLORS);

function clamp(value, min, max, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? Math.min(max, Math.max(min, number)) : fallback;
}

function color(value, fallback) {
  const text = String(value ?? '').trim();
  return /^#[0-9a-f]{6}$/i.test(text) ? text.toLowerCase() : fallback;
}

function cleanText(value, fallback, max = 48) {
  const text = String(value ?? '').replace(/[\u0000-\u001f]/g, '').trim().slice(0, max);
  return text || fallback;
}

export function defaultFxProfile(overrides = {}) {
  return normaliseFxProfile({ ...structuredClone(DEFAULT_FX_PROFILE), ...overrides });
}

export function normaliseFxProfile(value = {}) {
  const input = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  const profile = structuredClone(DEFAULT_FX_PROFILE);
  profile.version = FX_PROFILE_VERSION;

  for (const [key, options] of Object.entries(ENUMS)) {
    profile[key] = options.includes(input[key]) ? input[key] : profile[key];
  }
  for (const [key, [min, max]] of Object.entries(NUMBERS)) {
    profile[key] = clamp(input[key], min, max, profile[key]);
  }
  for (const key of BOOLEANS) {
    if (typeof input[key] === 'boolean') profile[key] = input[key];
  }

  // Thresholds describe adjacent bands. An imported file cannot make the
  // middle band start after the perfect band or silently disable a band by
  // reversing it.
  profile.quality_epic = Math.round(profile.quality_epic);
  profile.quality_near = Math.max(profile.quality_epic, Math.round(profile.quality_near));
  profile.quality_perfect = Math.max(profile.quality_near, Math.round(profile.quality_perfect));
  profile.particle_density = Math.round(profile.particle_density);
  profile.edge_inset = Math.round(profile.edge_inset);

  const paletteColors = PALETTES[profile.palette]?.colors ?? DEFAULT_COLORS;
  const incomingColors = input.colors && typeof input.colors === 'object' ? input.colors : {};
  profile.colors = {};
  for (const key of COLOR_KEYS) profile.colors[key] = color(incomingColors[key], paletteColors[key]);
  return profile;
}

export function applyPalette(profile, palette) {
  const picked = PALETTES[palette] ?? PALETTES['hero-siege'];
  return normaliseFxProfile({ ...profile, palette: Object.keys(PALETTES).find((key) => PALETTES[key] === picked) ?? 'hero-siege', colors: picked.colors });
}

export function makeFxPreset(name, profile, id = '') {
  const now = new Date().toISOString();
  return {
    id: cleanText(id, `fx-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`, 64),
    name: cleanText(name, 'My FX preset'),
    created_at: now,
    updated_at: now,
    fx: normaliseFxProfile(profile),
  };
}

export function normaliseFxPreset(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return {
    id: cleanText(value.id, `fx-${Math.random().toString(36).slice(2, 10)}`, 64),
    name: cleanText(value.name, 'Imported FX'),
    created_at: cleanText(value.created_at, new Date().toISOString(), 40),
    updated_at: cleanText(value.updated_at, new Date().toISOString(), 40),
    fx: normaliseFxProfile(value.fx ?? value.profile),
  };
}

export function normaliseFxPresets(values) {
  const seen = new Set();
  const presets = [];
  for (const value of Array.isArray(values) ? values : []) {
    const preset = normaliseFxPreset(value);
    if (!preset || seen.has(preset.id)) continue;
    seen.add(preset.id);
    presets.push(preset);
    if (presets.length >= FX_PRESET_LIMIT) break;
  }
  return presets;
}

export const STARTER_FX_PRESETS = Object.freeze([
  {
    id: 'starter-hero-siege',
    name: 'Hero Siege Classic',
    fx: defaultFxProfile(),
  },
  {
    id: 'starter-frost-orb',
    name: 'Frost Orb',
    fx: defaultFxProfile({
      palette: 'frost',
      colors: PALETTES.frost.colors,
      entrance: 'rift',
      particle_density: 72,
      particle_trails: true,
      glow_intensity: 1.2,
      beam_enabled: true,
      shockwave_enabled: true,
    }),
  },
  {
    id: 'starter-demonic',
    name: 'Demonic Rift',
    fx: defaultFxProfile({
      palette: 'demonic',
      colors: PALETTES.demonic.colors,
      entrance: 'slam',
      shade: 0.72,
      screen_flash: 0.16,
      beam_enabled: true,
      shockwave_enabled: true,
    }),
  },
  {
    id: 'starter-clean-neon',
    name: 'Clean Neon',
    fx: defaultFxProfile({
      palette: 'neon',
      colors: PALETTES.neon.colors,
      layout: 'compact',
      shade: 0.28,
      particle_density: 20,
      beam_enabled: false,
    }),
  },
  {
    id: 'starter-performance',
    name: 'Low GPU',
    fx: defaultFxProfile({
      layout: 'compact',
      entrance: 'fade',
      shade: 0.32,
      glow_intensity: 0.65,
      beam_enabled: false,
      shockwave_enabled: false,
      particles_enabled: false,
      stat_fx_enabled: false,
    }),
  },
]);

export function fxBundle(presets, current) {
  return {
    app: 'hs-tracker',
    kind: 'fx-presets',
    version: 1,
    exported_at: new Date().toISOString(),
    current: normaliseFxProfile(current),
    presets: normaliseFxPresets(presets),
  };
}

export function parseFxBundle(value) {
  const input = typeof value === 'string' ? JSON.parse(value) : value;
  if (!input || input.app !== 'hs-tracker' || input.kind !== 'fx-presets') {
    throw new Error('not an HS Tracker FX preset file');
  }
  return {
    current: normaliseFxProfile(input.current),
    presets: normaliseFxPresets(input.presets),
  };
}
