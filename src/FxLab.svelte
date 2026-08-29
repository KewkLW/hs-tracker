<script>
  import { invoke, listen } from './bridge.js';
  import { art } from './skin.svelte.js';
  import {
    FX_PRESET_LIMIT,
    PALETTES,
    STARTER_FX_PRESETS,
    applyPalette,
    defaultFxProfile,
    fxBundle,
    makeFxPreset,
    normaliseFxPresets,
    normaliseFxProfile,
    parseFxBundle,
  } from './fx-presets.js';

  let settings = $state(null);
  let fx = $state(defaultFxProfile());
  let presets = $state([]);
  let selected = $state('starter-hero-siege');
  let presetName = $state('Hero Siege Classic');
  let notice = $state('');
  let testing = $state('');
  let fileInput = $state(null);
  let saveTimer = null;
  let pendingSettings = null;
  let noticeTimer = null;
  let armedDelete = $state(false);
  let deleteTimer = null;

  const LAYOUTS = [
    ['cinematic', 'Cinematic', 'Full beam, particles and centered item card'],
    ['compact', 'Compact', 'A tighter alert for busy farming'],
    ['edge', 'Screen edge', 'Slides in from a chosen edge'],
  ];
  const ENTRANCES = [
    ['rise', 'Rise'],
    ['slam', 'Slam'],
    ['rift', 'Rift open'],
    ['fade', 'Soft fade'],
  ];
  const EDGE_POSITIONS = [
    ['top', 'Top'],
    ['bottom', 'Bottom'],
    ['left', 'Left'],
    ['right', 'Right'],
  ];
  const COLOR_ROWS = [
    ['ordinary', 'Ordinary'],
    ['high_roll', 'High roll'],
    ['near_perfect', 'Near perfect'],
    ['perfect', 'Perfect'],
    ['stat', 'Custom stat'],
    ['combined', 'Combined'],
  ];

  let selectedPreset = $derived(
    STARTER_FX_PRESETS.find((preset) => preset.id === selected)
      ?? presets.find((preset) => preset.id === selected)
      ?? null,
  );
  let selectedIsStarter = $derived(selected.startsWith('starter-'));
  let dirty = $derived.by(() => {
    if (!selectedPreset) return true;
    return JSON.stringify(normaliseFxProfile(fx)) !== JSON.stringify(normaliseFxProfile(selectedPreset.fx));
  });

  function flash(message) {
    clearTimeout(noticeTimer);
    notice = message;
    noticeTimer = setTimeout(() => (notice = ''), 5000);
  }

  function uniqueName(wanted, exceptId = '') {
    const base = String(wanted || 'My FX preset').trim().slice(0, 48) || 'My FX preset';
    const used = new Set(presets.filter((preset) => preset.id !== exceptId).map((preset) => preset.name.toLowerCase()));
    if (!used.has(base.toLowerCase())) return base;
    let number = 2;
    while (used.has(`${base} ${number}`.toLowerCase())) number += 1;
    return `${base} ${number}`;
  }

  function profileFromSettings(value) {
    // Older settings files acquire an empty `flourish_fx` object through
    // serde's defaults. Empty means "not configured yet", not "replace the
    // player's existing scale/shade/duration with the new profile defaults".
    if (
      value?.flourish_fx
      && typeof value.flourish_fx === 'object'
      && !Array.isArray(value.flourish_fx)
      && Object.keys(value.flourish_fx).length > 0
    ) {
      return normaliseFxProfile(value.flourish_fx);
    }
    // The three controls that existed before the lab seed its first profile.
    return defaultFxProfile({
      scale: value?.flourish_scale ?? 1,
      shade: value?.flourish_shade ?? 0.55,
      duration_s: value?.flourish_secs ?? 6,
    });
  }

  function hydrate(value) {
    if (!value) return;
    settings = value;
    fx = profileFromSettings(value);
    presets = normaliseFxPresets(value.flourish_fx_presets);
    const remembered = String(value.flourish_fx_preset ?? '');
    if (STARTER_FX_PRESETS.some((preset) => preset.id === remembered) || presets.some((preset) => preset.id === remembered)) {
      selected = remembered;
    } else if (presets.length) {
      selected = presets[0].id;
    }
    presetName = (STARTER_FX_PRESETS.find((preset) => preset.id === selected) ?? presets.find((preset) => preset.id === selected))?.name ?? 'My FX preset';
  }

  $effect(() => {
    invoke('get_settings').then(hydrate).catch(() => {});
    const unsub = listen('settings-changed', (event) => {
      // Keep an edit that is waiting on this page's debounce, but take every
      // unrelated setting changed by the tray or another tab.
      if (saveTimer && settings) {
        const own = {
          flourish_fx: $state.snapshot(fx),
          flourish_fx_presets: $state.snapshot(presets),
          flourish_fx_preset: selected,
          flourish_scale: fx.scale,
          flourish_shade: fx.shade,
          flourish_secs: fx.duration_s,
        };
        settings = { ...event.payload, ...own };
        // `persist` debounces a complete settings snapshot. Refresh that
        // pending snapshot too, or the timer would write the pre-event copy
        // back and undo whichever Twitch/Alerts/Settings edit just arrived.
        pendingSettings = $state.snapshot(settings);
      } else {
        hydrate(event.payload);
      }
    });
    return () => {
      // A tab change destroys this component. The old cleanup discarded any
      // slider/input edit still inside the 150 ms debounce, so a quick click on
      // another tab made the last change appear to save and then vanish. Send
      // the already-merged snapshot before leaving instead.
      if (saveTimer && pendingSettings) {
        clearTimeout(saveTimer);
        const snapshot = pendingSettings;
        saveTimer = null;
        pendingSettings = null;
        invoke('save_settings', { settings: snapshot }).catch(() => {});
      } else {
        clearTimeout(saveTimer);
      }
      clearTimeout(noticeTimer);
      clearTimeout(deleteTimer);
      unsub.then((stop) => stop());
    };
  });

  function persist() {
    if (!settings) return;
    fx = normaliseFxProfile($state.snapshot(fx));
    settings.flourish_fx = $state.snapshot(fx);
    settings.flourish_fx_presets = $state.snapshot(presets);
    settings.flourish_fx_preset = selected;

    // Backward compatibility: old builds and the current window-sizing code
    // still read these three top-level fields.
    settings.flourish_scale = fx.scale;
    settings.flourish_shade = fx.shade;
    settings.flourish_secs = fx.duration_s;

    clearTimeout(saveTimer);
    pendingSettings = $state.snapshot(settings);
    saveTimer = setTimeout(() => {
      saveTimer = null;
      const snapshot = pendingSettings;
      pendingSettings = null;
      invoke('save_settings', { settings: snapshot }).catch((error) => flash(String(error)));
    }, 150);
  }

  function toggle(key) {
    fx[key] = !fx[key];
    persist();
  }

  function choosePalette(value) {
    fx = applyPalette($state.snapshot(fx), value);
    persist();
  }

  function choosePreset(event) {
    selected = event.currentTarget.value;
    presetName = selectedPreset?.name ?? 'My FX preset';
    armedDelete = false;
  }

  function loadPreset() {
    if (!selectedPreset) return;
    fx = normaliseFxProfile(selectedPreset.fx);
    presetName = selectedPreset.name;
    persist();
    flash(`Loaded ${selectedPreset.name}`);
  }

  function saveNew() {
    if (presets.length >= FX_PRESET_LIMIT) {
      flash(`The preset limit is ${FX_PRESET_LIMIT}`);
      return;
    }
    const made = makeFxPreset(uniqueName(presetName), $state.snapshot(fx));
    presets = [...presets, made];
    selected = made.id;
    presetName = made.name;
    persist();
    flash(`Saved ${made.name}`);
  }

  function updatePreset() {
    if (!selectedPreset || selectedIsStarter) return;
    const now = new Date().toISOString();
    presets = presets.map((preset) => preset.id === selected
      ? { ...preset, fx: normaliseFxProfile($state.snapshot(fx)), updated_at: now }
      : preset);
    persist();
    flash(`Updated ${selectedPreset.name}`);
  }

  function renamePreset() {
    if (!selectedPreset || selectedIsStarter) return;
    const name = uniqueName(presetName, selected);
    presets = presets.map((preset) => preset.id === selected
      ? { ...preset, name, updated_at: new Date().toISOString() }
      : preset);
    presetName = name;
    persist();
    flash(`Renamed preset to ${name}`);
  }

  function duplicatePreset() {
    if (presets.length >= FX_PRESET_LIMIT) {
      flash(`The preset limit is ${FX_PRESET_LIMIT}`);
      return;
    }
    const name = uniqueName(`${selectedPreset?.name ?? presetName} copy`);
    // Duplicate the live controls, not the last saved version: it is a useful
    // way to branch a preset after experimenting without overwriting it.
    const made = makeFxPreset(name, $state.snapshot(fx));
    presets = [...presets, made];
    selected = made.id;
    presetName = made.name;
    persist();
    flash(`Created ${made.name}`);
  }

  function deletePreset() {
    if (!selectedPreset || selectedIsStarter) return;
    clearTimeout(deleteTimer);
    if (!armedDelete) {
      armedDelete = true;
      deleteTimer = setTimeout(() => (armedDelete = false), 4000);
      return;
    }
    const oldName = selectedPreset.name;
    presets = presets.filter((preset) => preset.id !== selected);
    selected = presets[0]?.id ?? 'starter-hero-siege';
    presetName = (presets[0] ?? STARTER_FX_PRESETS[0]).name;
    armedDelete = false;
    persist();
    flash(`Deleted ${oldName}`);
  }

  function exportPresets() {
    const data = JSON.stringify(fxBundle($state.snapshot(presets), $state.snapshot(fx)), null, 2);
    const blob = new Blob([data], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'hs-tracker-fx-presets.json';
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    flash(`Exported ${presets.length} saved preset${presets.length === 1 ? '' : 's'}`);
  }

  async function importPresets(event) {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = '';
    if (!file) return;
    try {
      const incoming = parseFxBundle(await file.text());
      const room = Math.max(0, FX_PRESET_LIMIT - presets.length);
      const usedIds = new Set(presets.map((preset) => preset.id));
      const additions = [];
      for (const original of incoming.presets.slice(0, room)) {
        const id = usedIds.has(original.id) ? '' : original.id;
        const made = makeFxPreset(uniqueName(original.name), original.fx, id);
        usedIds.add(made.id);
        additions.push(made);
      }
      presets = [...presets, ...additions];
      fx = incoming.current;
      if (additions.length) {
        selected = additions[0].id;
        presetName = additions[0].name;
      }
      persist();
      flash(`Imported ${additions.length} preset${additions.length === 1 ? '' : 's'} and applied the exported look`);
    } catch (error) {
      flash(String(error?.message ?? error));
    }
  }

  function testVisual(kind) {
    const profile = $state.snapshot(fx);
    let preview = {
      rarity: 'Set',
      name: "Gladiator's Skullet",
      tier: 5,
      item_type: 0,
      weapon_type: 0,
      high_roll: true,
      roll_percent: 78,
      fx: profile,
      fx_preview: kind,
    };
    if (kind === 'near') preview = { ...preview, rarity: 'Angelic', name: 'Lunar Prophecy', roll_percent: Math.max(96, fx.quality_near) };
    if (kind === 'perfect') preview = { ...preview, rarity: 'Satanic', name: 'Perfect Rift Orb', tier: 6, roll_percent: 100 };
    if (kind === 'stat') {
      preview = {
        ...preview,
        rarity: 'Satanic',
        name: 'Rift Vectors',
        tier: 6,
        stat_matches: [{ stat_id: 70, stat: 'projectile_speed', actual: 3, op: '>', target: 2 }],
      };
    }
    testing = kind;
    invoke('test_flourish', { preview })
      .catch((error) => flash(String(error)))
      .finally(() => setTimeout(() => { if (testing === kind) testing = ''; }, 450));
  }
</script>

<div class="lab">
  {#if settings}
    <div class="mast" style:border-image-source="url({art('chip_dark')})">
      <div class="mast-copy">
        <div class="title">FX Lab</div>
        <div class="hint">Shape the transparent drop overlay, test it safely, then save the look as a reusable preset.</div>
      </div>
      <div class="tests" aria-label="test visuals">
        <button class="btn test" class:on={testing === 'high'} onclick={() => testVisual('high')}>High roll</button>
        <button class="btn test" class:on={testing === 'near'} onclick={() => testVisual('near')}>Near perfect</button>
        <button class="btn test perfect" class:on={testing === 'perfect'} onclick={() => testVisual('perfect')}>Perfect</button>
        <button class="btn test stat" class:on={testing === 'stat'} onclick={() => testVisual('stat')}>Stat alert</button>
      </div>
    </div>

    <section class="section presets" style:border-image-source="url({art('chip_dark')})">
      <div class="sechead">Presets <span class:changed={dirty}>{dirty ? '· modified' : '· saved'}</span></div>
      <div class="preset-row">
        <select class="picker preset-picker" value={selected} onchange={choosePreset} aria-label="FX preset">
          <optgroup label="Starter looks">
            {#each STARTER_FX_PRESETS as preset}
              <option value={preset.id}>{preset.name}</option>
            {/each}
          </optgroup>
          {#if presets.length}
            <optgroup label="My presets">
              {#each presets as preset}
                <option value={preset.id}>{preset.name}</option>
              {/each}
            </optgroup>
          {/if}
        </select>
        <button class="btn" onclick={loadPreset}>Load</button>
        <button class="btn" onclick={duplicatePreset}>Duplicate</button>
      </div>
      <div class="preset-row">
        <input class="field" maxlength="48" bind:value={presetName} placeholder="Preset name" aria-label="preset name" />
        <button class="btn" onclick={saveNew}>Save new</button>
        <button class="btn" disabled={selectedIsStarter} onclick={updatePreset}>Update</button>
        <button class="btn" disabled={selectedIsStarter} onclick={renamePreset}>Rename</button>
        <button class="btn danger" class:armed={armedDelete} disabled={selectedIsStarter} onclick={deletePreset}>
          {armedDelete ? 'Delete?' : 'Delete'}
        </button>
      </div>
      <div class="preset-row minor">
        <button class="link" onclick={() => fileInput?.click()}>Import preset pack</button>
        <button class="link" onclick={exportPresets}>Export preset pack</button>
        <input class="hidden" bind:this={fileInput} type="file" accept="application/json,.json" onchange={importPresets} />
        <span class="capacity">{presets.length}/{FX_PRESET_LIMIT} saved</span>
        {#if notice}<span class="notice" title={notice}>{notice}</span>{/if}
      </div>
    </section>

    <div class="columns">
      <div class="column">
        <section class="section" style:border-image-source="url({art('chip_dark')})">
          <div class="sechead">Layout & motion</div>
          <div class="choice-grid layouts">
            {#each LAYOUTS as [value, label, description]}
              <button
                class="choice"
                class:on={fx.layout === value}
                onclick={() => { fx.layout = value; persist(); }}
                title={description}
              >
                <span>{label}</span><small>{description}</small>
              </button>
            {/each}
          </div>

          <label class="line">
            <span class="name">Entrance</span>
            <select class="picker" bind:value={fx.entrance} onchange={persist}>
              {#each ENTRANCES as [value, label]}<option value={value}>{label}</option>{/each}
            </select>
          </label>
          <label class="line">
            <span class="name">Duration</span>
            <input type="range" min="2" max="12" step="0.5" bind:value={fx.duration_s} oninput={persist} />
            <span class="value">{fx.duration_s.toFixed(1)}s</span>
          </label>
          <label class="line">
            <span class="name">Scale</span>
            <input type="range" min="50" max="200" value={Math.round(fx.scale * 100)} oninput={(event) => { fx.scale = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.scale * 100)}%</span>
          </label>
          <label class="line">
            <span class="name">Backdrop</span>
            <input type="range" min="0" max="90" value={Math.round(fx.shade * 100)} oninput={(event) => { fx.shade = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.shade * 100)}%</span>
          </label>
          <label class="line">
            <span class="name">Text size</span>
            <input type="range" min="70" max="160" value={Math.round(fx.font_scale * 100)} oninput={(event) => { fx.font_scale = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.font_scale * 100)}%</span>
          </label>

          {#if fx.layout === 'edge'}
            <div class="subsection">
              <div class="minihead">Screen edge</div>
              <div class="segmented">
                {#each EDGE_POSITIONS as [value, label]}
                  <button class:on={fx.edge_position === value} onclick={() => { fx.edge_position = value; persist(); }}>{label}</button>
                {/each}
              </div>
              <label class="line">
                <span class="name">Inset</span>
                <input type="range" min="0" max="200" bind:value={fx.edge_inset} oninput={persist} />
                <span class="value">{fx.edge_inset}px</span>
              </label>
            </div>
          {/if}
        </section>

        <section class="section" style:border-image-source="url({art('chip_dark')})">
          <div class="sechead">Light & particles</div>
          <button class="tick" onclick={() => toggle('glow_enabled')}>
            <img src={fx.glow_enabled ? art('check_on') : art('check_off')} alt="" />
            <span>Animated ground glow</span>
          </button>
          <label class="line inset" class:off={!fx.glow_enabled}>
            <span class="name">Glow power</span>
            <input disabled={!fx.glow_enabled} type="range" min="0" max="150" value={Math.round(fx.glow_intensity * 100)} oninput={(event) => { fx.glow_intensity = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.glow_intensity * 100)}%</span>
          </label>
          <div class="tick-grid">
            <button class="tick" onclick={() => toggle('beam_enabled')}>
              <img src={fx.beam_enabled ? art('check_on') : art('check_off')} alt="" /><span>Loot beam</span>
            </button>
            <button class="tick" onclick={() => toggle('shockwave_enabled')}>
              <img src={fx.shockwave_enabled ? art('check_on') : art('check_off')} alt="" /><span>Shockwave</span>
            </button>
            <button class="tick" onclick={() => toggle('particle_trails')}>
              <img src={fx.particle_trails ? art('check_on') : art('check_off')} alt="" /><span>Particle trails</span>
            </button>
          </div>
          <button class="tick" onclick={() => toggle('particles_enabled')}>
            <img src={fx.particles_enabled ? art('check_on') : art('check_off')} alt="" />
            <span>Particle field</span>
          </button>
          <label class="line inset" class:off={!fx.particles_enabled}>
            <span class="name">Density</span>
            <input disabled={!fx.particles_enabled} type="range" min="0" max="100" bind:value={fx.particle_density} oninput={persist} />
            <span class="value">{fx.particle_density}%</span>
          </label>
          <label class="line inset" class:off={!fx.particles_enabled}>
            <span class="name">Particle size</span>
            <input disabled={!fx.particles_enabled} type="range" min="50" max="200" value={Math.round(fx.particle_size * 100)} oninput={(event) => { fx.particle_size = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.particle_size * 100)}%</span>
          </label>
          <label class="line inset" class:off={!fx.particles_enabled}>
            <span class="name">Particle speed</span>
            <input disabled={!fx.particles_enabled} type="range" min="50" max="200" value={Math.round(fx.particle_speed * 100)} oninput={(event) => { fx.particle_speed = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.particle_speed * 100)}%</span>
          </label>
          <label class="line">
            <span class="name">Screen flash</span>
            <input disabled={fx.reduce_motion} type="range" min="0" max="100" value={Math.round(fx.screen_flash * 100)} oninput={(event) => { fx.screen_flash = event.currentTarget.value / 100; persist(); }} />
            <span class="value">{Math.round(fx.screen_flash * 100)}%</span>
          </label>
        </section>

        <section class="section" style:border-image-source="url({art('chip_dark')})">
          <div class="sechead">Text & accessibility</div>
          <div class="tick-grid two">
            {#each [
              ['show_heading', 'Alert heading'],
              ['show_item_name', 'Item name'],
              ['show_tier', 'Item tier'],
              ['show_stat', 'Matched stat'],
            ] as [key, label]}
              <button class="tick" onclick={() => toggle(key)}>
                <img src={fx[key] ? art('check_on') : art('check_off')} alt="" /><span>{label}</span>
              </button>
            {/each}
          </div>
          <button class="tick" onclick={() => toggle('reduce_motion')}>
            <img src={fx.reduce_motion ? art('check_on') : art('check_off')} alt="" />
            <span>Reduce motion and disable flashing</span>
          </button>
          <div class="note">Reduced motion keeps the information and colors, but swaps movement-heavy entrances for a short fade.</div>
        </section>
      </div>

      <div class="column">
        <section class="section quality" style:border-image-source="url({art('chip_dark')})">
          <div class="sechead">Roll-quality escalation</div>
          <button class="tick" onclick={() => toggle('quality_escalation')}>
            <img src={fx.quality_escalation ? art('check_on') : art('check_off')} alt="" />
            <span>Escalate the effect as rolls approach perfect</span>
          </button>
          <div class="bands" class:off={!fx.quality_escalation}>
            <div class="band-row">
              <span class="swatch" style:background={fx.colors.high_roll}></span>
              <span class="band-name">High roll</span>
              <span class="band-range">alert threshold–{Math.max(0, fx.quality_epic - 1)}%</span>
            </div>
            <label class="band-row">
              <span class="swatch" style:background={fx.colors.near_perfect}></span>
              <span class="band-name">Epic burst</span>
              <input class="number" disabled={!fx.quality_escalation} type="number" min="0" max="100" bind:value={fx.quality_epic} onchange={persist} />
              <span>%+</span>
            </label>
            <label class="band-row">
              <span class="swatch" style:background={fx.colors.perfect}></span>
              <span class="band-name">Near perfect</span>
              <input class="number" disabled={!fx.quality_escalation} type="number" min="0" max="100" bind:value={fx.quality_near} onchange={persist} />
              <span>%+</span>
            </label>
            <label class="band-row perfect-band">
              <span class="swatch" style:background={fx.colors.perfect}></span>
              <span class="band-name">Perfect jackpot</span>
              <input class="number" disabled={!fx.quality_escalation} type="number" min="0" max="100" bind:value={fx.quality_perfect} onchange={persist} />
              <span>%</span>
            </label>
          </div>
          <div class="note">The normal roll-alert threshold still decides what qualifies. These bands only make exceptional results look more exceptional.</div>
        </section>

        <section class="section" style:border-image-source="url({art('chip_dark')})">
          <div class="sechead">Stat-reactive effects</div>
          <button class="tick" onclick={() => toggle('stat_fx_enabled')}>
            <img src={fx.stat_fx_enabled ? art('check_on') : art('check_off')} alt="" />
            <span>Choose accents from the stats that matched</span>
          </button>
          <div class="effect-cards" class:off={!fx.stat_fx_enabled}>
            <button class="effect-card" class:on={fx.projectile_trails} disabled={!fx.stat_fx_enabled} onclick={() => toggle('projectile_trails')}>
              <span class="effect-icon projectile">➜</span><span>Projectile streaks</span><small>Projectile speed</small>
            </button>
            <button class="effect-card" class:on={fx.vitality_pulse} disabled={!fx.stat_fx_enabled} onclick={() => toggle('vitality_pulse')}>
              <span class="effect-icon vitality">♥</span><span>Heartbeat pulse</span><small>Vitality & life</small>
            </button>
            <button class="effect-card" class:on={fx.crushing_shockwave} disabled={!fx.stat_fx_enabled} onclick={() => toggle('crushing_shockwave')}>
              <span class="effect-icon crushing">✹</span><span>Impact ring</span><small>Crushing blow</small>
            </button>
            <button class="effect-card" class:on={fx.socket_orbit} disabled={!fx.stat_fx_enabled} onclick={() => toggle('socket_orbit')}>
              <span class="effect-icon sockets">◉</span><span>Socket orbit</span><small>Socket count</small>
            </button>
          </div>
          <div class="note">A custom alert can match several rules. Their accents combine into one alert rather than opening several overlays.</div>
        </section>

        <section class="section palette" style:border-image-source="url({art('chip_dark')})">
          <div class="sechead">Palette</div>
          <label class="line">
            <span class="name">Starting palette</span>
            <select class="picker" value={fx.palette} onchange={(event) => choosePalette(event.currentTarget.value)}>
              {#each Object.entries(PALETTES) as [value, palette]}
                <option value={value}>{palette.label}</option>
              {/each}
            </select>
          </label>
          <div class="color-grid">
            {#each COLOR_ROWS as [key, label]}
              <label class="color-row">
                <input type="color" bind:value={fx.colors[key]} oninput={persist} aria-label={`${label} color`} />
                <span>{label}</span>
                <code>{fx.colors[key]}</code>
              </label>
            {/each}
          </div>
          <div class="palette-strip" aria-label="current palette">
            {#each COLOR_ROWS as [key]}<span style:background={fx.colors[key]}></span>{/each}
          </div>
        </section>
      </div>
    </div>
  {:else}
    <div class="empty">Loading FX settings…</div>
  {/if}
</div>

<style>
  @font-face {
    font-family: 'CookieRun Bold';
    src: url('./assets/fonts/cookierunbold.ttf') format('truetype');
  }

  .lab {
    box-sizing: border-box;
    width: 100%;
    height: 100%;
    overflow-y: auto;
    padding: 0 2px 10px 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-family: 'CookieRun Bold', sans-serif;
    font-size: 12px;
    color: var(--bone-6);
  }
  .lab::-webkit-scrollbar { width: 6px; }
  .lab::-webkit-scrollbar-thumb { background: var(--dim-1); border-radius: 3px; }

  .mast,
  .section {
    box-sizing: border-box;
    flex: none;
    border: 6px solid transparent;
    border-image-slice: 6 fill;
    border-image-width: 6px;
    image-rendering: pixelated;
  }
  .mast {
    min-height: 58px;
    padding: 5px 8px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .mast-copy { min-width: 190px; }
  .title { color: var(--bone-13); font-size: 15px; }
  .hint,
  .note { color: var(--dim-2); font-size: 10px; line-height: 1.4; }
  .tests { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 5px; }
  .test.on { filter: brightness(1.45); }
  .test.perfect { color: #fff5c2; }
  .test.stat { color: #7be9ff; }

  .section {
    padding: 5px 7px 7px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    min-width: 0;
  }
  .sechead,
  .minihead {
    color: var(--edge-2b);
    font-size: 10px;
    letter-spacing: 0.3px;
    text-transform: uppercase;
  }
  .sechead span { color: #45c15a; }
  .sechead span.changed { color: var(--gold-1); }

  .columns {
    display: grid;
    grid-template-columns: repeat(2, minmax(300px, 1fr));
    gap: 8px;
    align-items: start;
  }
  .column { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
  @media (max-width: 900px) {
    .columns { grid-template-columns: minmax(0, 1fr); }
    .mast { align-items: flex-start; flex-direction: column; }
    .tests { justify-content: flex-start; }
  }

  .preset-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 5px;
  }
  .preset-picker { min-width: 180px; }
  .preset-row .field { flex: 1 1 170px; }
  .preset-row.minor { min-height: 20px; }
  .capacity { margin-left: auto; color: var(--edge-5); font-size: 10px; }
  .notice {
    flex: 1 1 180px;
    min-width: 0;
    color: #45c15a;
    font-size: 10px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .hidden { display: none; }

  .choice-grid { display: grid; gap: 4px; }
  .layouts { grid-template-columns: repeat(3, minmax(0, 1fr)); }
  .choice {
    min-width: 0;
    min-height: 54px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
    padding: 6px;
    font: inherit;
    color: var(--bone-4);
    background: rgba(0, 0, 0, 0.24);
    border: 1px solid var(--ground-10);
    cursor: pointer;
    text-align: left;
  }
  .choice:hover { color: var(--bone-13); border-color: var(--edge-4); }
  .choice.on { color: var(--bone-13); border-color: var(--edge-4); background: rgba(150, 37, 56, 0.3); }
  .choice small { color: var(--dim-2); font: inherit; font-size: 9px; line-height: 1.25; }

  .line {
    display: flex;
    align-items: center;
    gap: 7px;
    min-height: 25px;
  }
  .line .name { width: 92px; flex: none; }
  .line .picker { max-width: 260px; }
  .line.inset { padding-left: 24px; }
  .line.off,
  .bands.off,
  .effect-cards.off { opacity: 0.45; }
  .value {
    width: 44px;
    flex: none;
    color: var(--edge-2b);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  input[type='range'] {
    flex: 1 1 auto;
    min-width: 60px;
    max-width: 280px;
    height: 14px;
    appearance: none;
    -webkit-appearance: none;
    background: none;
    cursor: pointer;
  }
  input[type='range']::-webkit-slider-runnable-track {
    height: 4px;
    background: var(--ground-7);
    border: 1px solid var(--ground-11);
  }
  input[type='range']::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 11px;
    height: 11px;
    margin-top: -5px;
    background: var(--bone-6);
    border: 1px solid var(--ground-7);
  }
  input[type='range']:hover:not(:disabled)::-webkit-slider-thumb { background: var(--bone-13); }
  input[type='range']:disabled { opacity: 0.4; cursor: default; }

  .picker {
    flex: 1 1 auto;
    min-width: 0;
    box-sizing: border-box;
    height: 25px;
    appearance: none;
    -webkit-appearance: none;
    font: inherit;
    font-size: 11px;
    color: var(--bone-13);
    background-color: rgba(0, 0, 0, 0.35);
    background-image: linear-gradient(45deg, transparent 50%, var(--bone-6) 50%), linear-gradient(135deg, var(--bone-6) 50%, transparent 50%);
    background-position: calc(100% - 12px) 50%, calc(100% - 7px) 50%;
    background-size: 5px 5px, 5px 5px;
    background-repeat: no-repeat;
    border: 1px solid var(--ground-10);
    padding: 3px 22px 3px 6px;
    cursor: pointer;
  }
  .picker:hover,
  .picker:focus { outline: none; border-color: var(--edge-4); }
  .picker option,
  .picker optgroup { background: var(--ground-7); color: var(--bone-9); }

  .field,
  .number {
    box-sizing: border-box;
    height: 25px;
    min-width: 0;
    font: inherit;
    font-size: 11px;
    color: var(--bone-13);
    background: rgba(0, 0, 0, 0.35);
    border: 1px solid var(--ground-10);
    outline: none;
    padding: 2px 6px;
  }
  .field:hover,
  .field:focus,
  .number:hover,
  .number:focus { border-color: var(--edge-4); }
  .number { width: 56px; text-align: right; font-variant-numeric: tabular-nums; }

  .tick,
  .link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-size: 11px;
    color: inherit;
    background: none;
    border: none;
    padding: 1px 0;
    cursor: pointer;
    text-align: left;
  }
  .tick:hover,
  .link:hover { color: var(--bone-13); }
  .tick img { width: 18px; height: 18px; image-rendering: pixelated; }
  .tick-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 2px 8px; }
  .tick-grid.two { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .link { color: var(--edge-2b); font-size: 10px; text-decoration: underline; text-decoration-color: var(--ground-11); }

  .subsection {
    margin-top: 2px;
    padding: 6px 4px 2px;
    border-top: 1px solid var(--ground-10);
  }
  .segmented { display: flex; gap: 3px; margin: 5px 0; }
  .segmented button {
    flex: 1 1 auto;
    font: inherit;
    font-size: 10px;
    color: var(--bone-3);
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--ground-10);
    padding: 3px 6px;
    cursor: pointer;
  }
  .segmented button.on { color: var(--bone-13); border-color: var(--edge-4); background: rgba(150, 37, 56, 0.35); }

  .bands { display: flex; flex-direction: column; gap: 2px; }
  .band-row {
    min-height: 27px;
    display: grid;
    grid-template-columns: 16px minmax(90px, 1fr) auto 18px;
    align-items: center;
    gap: 6px;
    padding: 1px 5px;
    background: rgba(0, 0, 0, 0.14);
  }
  .swatch { width: 11px; height: 11px; border: 1px solid rgba(255, 255, 255, 0.25); box-shadow: 0 0 7px currentColor; }
  .band-name { color: var(--bone-9); }
  .band-range { grid-column: 3 / 5; color: var(--dim-2); font-size: 9px; }
  .perfect-band { color: #fff5c2; }

  .effect-cards { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 4px; }
  .effect-card {
    min-width: 0;
    display: grid;
    grid-template-columns: 25px minmax(0, 1fr);
    grid-template-rows: auto auto;
    align-items: center;
    padding: 5px;
    font: inherit;
    font-size: 11px;
    color: var(--bone-4);
    text-align: left;
    background: rgba(0, 0, 0, 0.2);
    border: 1px solid var(--ground-10);
    cursor: pointer;
  }
  .effect-card.on { color: var(--bone-13); border-color: var(--edge-4); background: rgba(150, 37, 56, 0.24); }
  .effect-card:disabled { cursor: default; }
  .effect-card small { grid-column: 2; color: var(--dim-2); font: inherit; font-size: 9px; }
  .effect-icon { grid-row: 1 / 3; font-size: 18px; text-align: center; text-shadow: 0 0 9px currentColor; }
  .effect-icon.projectile { color: #5fdcff; }
  .effect-icon.vitality { color: #ff5d70; }
  .effect-icon.crushing { color: #ffc45d; }
  .effect-icon.sockets { color: #c287ff; }

  .color-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 3px 10px; }
  .color-row { display: grid; grid-template-columns: 24px minmax(70px, 1fr) auto; align-items: center; gap: 5px; }
  .color-row input[type='color'] {
    width: 22px;
    height: 20px;
    padding: 0;
    border: 1px solid var(--ground-10);
    background: none;
    cursor: pointer;
  }
  .color-row code { color: var(--dim-2); font: inherit; font-size: 9px; text-transform: uppercase; }
  .palette-strip { height: 5px; display: flex; margin-top: 2px; }
  .palette-strip span { flex: 1 1 auto; box-shadow: 0 0 8px currentColor; }

  .btn {
    box-sizing: border-box;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 25px;
    flex: none;
    font: inherit;
    font-size: 10px;
    line-height: 1;
    color: var(--bone-13);
    border: 6px solid transparent;
    border-image-source: var(--btn);
    border-image-slice: 6 fill;
    border-image-width: 6px;
    padding: 0 7px;
    cursor: pointer;
    image-rendering: pixelated;
  }
  .btn:hover:not(:disabled) { border-image-source: var(--btn-hover); }
  .btn:active:not(:disabled) { border-image-source: var(--btn-down); }
  .btn:disabled { opacity: 0.38; cursor: default; }
  .btn.danger.armed { color: #ffd1d1; filter: saturate(1.6) brightness(1.15); }

  .empty { padding: 20px; color: var(--dim-2); text-align: center; }
</style>
