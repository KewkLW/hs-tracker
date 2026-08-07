<script>
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { RARITIES, soundUrl, play } from './audio.js';

  import panelBg from './assets/game/panel.png';
  import chipBg from './assets/game/chip_dark.png';
  import btnBg from './assets/game/button.png';
  import btnHoverBg from './assets/game/button_hover.png';
  import btnDownBg from './assets/game/button_down.png';
  import closeImg from './assets/game/close.png';
  import closeHoverImg from './assets/game/close_hover.png';
  import headerBg from './assets/game/header.png';
  import checkOff from './assets/game/check_off.png';
  import checkOn from './assets/game/check_on.png';

  const LABELS = {
    satanic: ['Satanic drop', 'c-sat'],
    set: ['Set drop', 'c-set'],
    heroic: ['Heroic drop', 'c-her'],
    angelic: ['Angelic drop', 'c-ang'],
    unholy: ['Unholy drop', 'c-unh'],
    mail: ['Mail reminder', 'c-gold'],
  };

  // rarities the alert filter can let through, in drop-value order
  const ALERT_RARITIES = ['Satanic', 'Set', 'Heroic', 'Angelic', 'Unholy'];
  // the game's grades; SS cannot be filtered out in game either
  const TIERS = [
    [0, 'any'],
    [1, 'D'],
    [2, 'C'],
    [3, 'B'],
    [4, 'A'],
    [5, 'S'],
    [6, 'SS'],
  ];

  function toggleAlert(rarity) {
    const on = new Set(settings.alerts ?? []);
    on.has(rarity) ? on.delete(rarity) : on.add(rarity);
    settings.alerts = [...on];
    save();
  }

  let settings = $state(null);
  let custom = $state({});

  async function refreshStatus() {
    const next = {};
    for (const r of RARITIES) next[r] = await invoke('sound_status', { rarity: r }).catch(() => null);
    custom = next;
  }


  $effect(() => {
    invoke('get_settings').then((s) => (settings = s));
    refreshStatus();
    refreshRules();
    const unsubs = [listen('sounds-changed', refreshStatus)];
    return () => unsubs.forEach((u) => u.then((f) => f()));
  });

  let saveTimer;
  function save() {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => invoke('save_settings', { settings }).catch(() => {}), 150);
  }

  /// Sliders fire `input` while the DOM settles, which would persist a value
  /// the user never chose — only write on a real change.
  function setNumber(key, value) {
    if (!settings || !Number.isFinite(value) || settings[key] === value) return;
    settings[key] = value;
    save();
  }

  function toggle(rarity) {
    settings[rarity].enabled = !settings[rarity].enabled;
    save();
  }

  async function test(rarity) {
    play(await soundUrl(rarity), settings?.[rarity]?.volume ?? 0.7);
  }

  async function pickFile(rarity) {
    try {
      await invoke('pick_sound', { rarity });
    } catch (err) {
      alert(err);
    }
  }

  const SECTIONS = [
    ['session', 'Session timer, mail & reset'],
    ['gold', 'Gold'],
    ['xp', 'Experience'],
    ['items', 'Item counters'],
    ['zone', 'Satanic zone'],
    ['reset', 'Reset Stats button'],
  ];

  function toggleSection(id) {
    const hidden = new Set(settings.hidden ?? []);
    hidden.has(id) ? hidden.delete(id) : hidden.add(id);
    settings.hidden = [...hidden];
    save();
  }

  const rarityCls = {
    Satanic: 'c-sat',
    Set: 'c-set',
    Heroic: 'c-her',
    Angelic: 'c-ang',
    Unholy: 'c-unh',
  };

  const resetSound = (rarity) => invoke('clear_sound', { rarity }).catch(() => {});
  const hide = () => invoke('hide_settings');
</script>

<div class="panel" style:border-image-source="url({panelBg})" data-tauri-drag-region>
  <button class="close" onclick={hide} title="Close" aria-label="close">
    <img src={closeImg} alt="" class="close-normal" />
    <img src={closeHoverImg} alt="" class="close-hover" />
  </button>

  <div class="title" style:background-image="url({headerBg})" data-tauri-drag-region>
    <span>Settings</span>
  </div>

  <div class="body">
  {#if settings}
    <div class="section" style:border-image-source="url({chipBg})">
      <div class="line" data-tauri-drag-region>
        <span class="name">Opacity</span>
        <input
          type="range"
          min="30"
          max="100"
          value={Math.round((settings.opacity ?? 1) * 100)}
          oninput={(e) => setNumber('opacity', e.target.value / 100)}
        />
        <span class="pct">{Math.round((settings.opacity ?? 1) * 100)}%</span>
      </div>
      <div class="line" data-tauri-drag-region>
        <span class="name">Scale</span>
        <input
          type="range"
          min="60"
          max="150"
          value={Math.round((settings.scale ?? 1) * 100)}
          oninput={(e) => setNumber('scale', e.target.value / 100)}
        />
        <span class="pct">{Math.round((settings.scale ?? 1) * 100)}%</span>
      </div>
      <div class="line" data-tauri-drag-region>
        <button class="check" onclick={() => { settings.auto_show = !settings.auto_show; save(); }} aria-label="auto show">
          <img src={settings.auto_show ? checkOn : checkOff} alt="" />
        </button>
        <span class="opt">Show / hide the overlay with the game</span>
      </div>
      <div class="line" data-tauri-drag-region>
        <button class="check" onclick={() => { settings.autostart = !settings.autostart; save(); }} aria-label="autostart">
          <img src={settings.autostart ? checkOn : checkOff} alt="" />
        </button>
        <span class="opt">Start with Windows</span>
      </div>
      <div class="line" data-tauri-drag-region>
        <button class="check" onclick={() => { settings.ticker = !settings.ticker; save(); }} aria-label="ticker">
          <img src={settings.ticker ? checkOn : checkOff} alt="" />
        </button>
        <span class="opt">Drop ticker under the overlay</span>
      </div>
      <div class="line" data-tauri-drag-region>
        <button
          class="check"
          onclick={() => { settings.sound_on_ground = !settings.sound_on_ground; save(); }}
          aria-label="sound on ground"
        >
          <img src={settings.sound_on_ground ? checkOn : checkOff} alt="" />
        </button>
        <span class="opt">Alert when the item drops (off = when picked up)</span>
      </div>
      <div class="line" data-tauri-drag-region>
        <button class="check" onclick={() => { settings.debug_log = !settings.debug_log; save(); }} aria-label="debug">
          <img src={settings.debug_log ? checkOn : checkOff} alt="" />
        </button>
        <span class="opt">Log parsed packets to debug-capture.jsonl</span>
      </div>
      <div class="hotkeys" data-tauri-drag-region>
        Ctrl+Shift+O — show/hide · Ctrl+Shift+L — lock · Ctrl+Shift+R — reset stats
      </div>
    </div>

    <div class="section" style:border-image-source="url({chipBg})">
      <div class="sechead" data-tauri-drag-region>
        Loot filter — what gets a sound and a ticker line
      </div>
      <div class="grid">
        {#each ALERT_RARITIES as rarity}
          <button class="secopt" onclick={() => toggleAlert(rarity)}>
            <img src={(settings.alerts ?? []).includes(rarity) ? checkOn : checkOff} alt="" />
            <span class={rarityCls[rarity]}>{rarity}</span>
          </button>
        {/each}
      </div>
      <div class="line" data-tauri-drag-region>
        <span class="name">Min tier</span>
        <div class="tiers">
          {#each TIERS as [value, label]}
            <button
              class="tier"
              class:on={(settings.min_tier ?? 0) === value}
              onclick={() => setNumber('min_tier', value)}>{label}</button
            >
          {/each}
        </div>
      </div>
      <div class="hotkeys" data-tauri-drag-region>
        Counters still record everything — this only silences the alerts. Grades
        come from the item tables, so an item they do not list stays quiet while
        a minimum tier is set. Finds the server announces always sound.
      </div>
    </div>

    <div class="section" style:border-image-source="url({chipBg})">
      <div class="sechead" data-tauri-drag-region>Overlay sections</div>
      <div class="grid">
        {#each SECTIONS as [id, label]}
          <button class="secopt" onclick={() => toggleSection(id)}>
            <img src={(settings.hidden ?? []).includes(id) ? checkOff : checkOn} alt="" />
            <span>{label}</span>
          </button>
        {/each}
      </div>
    </div>
  {/if}

  {#if settings}
    {#each RARITIES as rarity}
      <div class="section" style:border-image-source="url({chipBg})">
        <div class="line" data-tauri-drag-region>
          <button class="check" onclick={() => toggle(rarity)} aria-label="toggle">
            <img src={settings[rarity].enabled ? checkOn : checkOff} alt="" />
          </button>
          <span class="name {LABELS[rarity][1]}">{LABELS[rarity][0]}</span>
          <input
            type="range"
            min="0"
            max="100"
            value={Math.round(settings[rarity].volume * 100)}
            oninput={(e) => {
              const v = e.target.value / 100;
              if (settings[rarity].volume === v) return;
              settings[rarity].volume = v;
              save();
            }}
            disabled={!settings[rarity].enabled}
          />
          <span class="pct">{Math.round(settings[rarity].volume * 100)}%</span>
        </div>
        <div class="line sub" data-tauri-drag-region>
          <span class="src" title={custom[rarity] ? `sounds\\${custom[rarity]}` : 'built-in sound'}>
            {custom[rarity] ?? 'built-in'}
          </span>
          <button
            class="btn sm"
            style:--btn="url({btnBg})"
            style:--btn-hover="url({btnHoverBg})"
            style:--btn-down="url({btnDownBg})"
            onclick={() => test(rarity)}>Test</button
          >
          <button
            class="btn sm"
            style:--btn="url({btnBg})"
            style:--btn-hover="url({btnHoverBg})"
            style:--btn-down="url({btnDownBg})"
            onclick={() => pickFile(rarity)}>Browse…</button
          >
          <button
            class="btn sm"
            style:--btn="url({btnBg})"
            style:--btn-hover="url({btnHoverBg})"
            style:--btn-down="url({btnDownBg})"
            disabled={!custom[rarity]}
            onclick={() => resetSound(rarity)}>Default</button
          >
        </div>
      </div>
    {/each}
  {/if}
  </div>
</div>

<style>
  @font-face {
    font-family: 'CookieRun Bold';
    src: url('./assets/fonts/cookierunbold.ttf') format('truetype');
  }

  :global(html, body) {
    margin: 0;
    height: 100%;
    background: transparent;
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
    cursor: default;
  }

  :global(img) {
    image-rendering: pixelated;
  }

  :global(#app) { height: 100%; }

  .panel {
    position: relative;
    box-sizing: border-box;
    width: 468px;
    height: 100%;
    border: 14px solid transparent;
    border-image-slice: 14 fill;
    border-image-width: 14px;
    border-image-repeat: stretch;
    image-rendering: pixelated;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-family: 'CookieRun Bold', sans-serif;
    font-size: 13px;
    color: #c3af75;
  }

  /* the list grows as features are added, so it scrolls instead of clipping */
  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-right: 2px;
  }
  .body::-webkit-scrollbar { width: 6px; }
  .body::-webkit-scrollbar-thumb { background: #4a3a3a; border-radius: 3px; }

  .title {
    height: 29px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    background-size: 100% 100%;
    background-repeat: no-repeat;
    image-rendering: pixelated;
    font-size: 13px;
  }

  .section {
    box-sizing: border-box;
    flex: none;
    border: 6px solid transparent;
    border-image-slice: 6 fill;
    border-image-width: 6px;
    image-rendering: pixelated;
    padding: 2px 6px 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 22px;
  }

  .line.sub {
    padding-left: 35px;
  }

  .check {
    width: 27px;
    height: 27px;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
    flex: none;
  }
  .check:hover { filter: brightness(1.25); }
  .check img { width: 27px; height: 27px; display: block; }

  .name { width: 108px; flex: none; }
  .opt { font-size: 12px; }

  .sechead {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: #9a8a68;
    padding: 2px 0 4px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px 10px;
  }
  .secopt {
    display: flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-size: 11px;
    color: #c3af75;
    background: none;
    border: none;
    cursor: pointer;
    padding: 1px 0;
    text-align: left;
  }
  .secopt img { width: 19px; height: 19px; flex: none; }
  .secopt:hover { color: #f0e0b0; }

  .tiers { display: flex; gap: 4px; flex: 1; }
  .tier {
    flex: 1;
    font: inherit;
    font-size: 11px;
    color: #8a7a5a;
    background: #241c1c;
    border: 1px solid #3a2e2e;
    border-radius: 2px;
    cursor: pointer;
    padding: 2px 0;
  }
  .tier:hover { color: #e0cc90; }
  .line.off .name,
  .tier:disabled {
    opacity: 0.45;
  }

  .tier.on {
    color: #f0e0b0;
    background: #4a1c22;
    border-color: #8a3a44;
  }
  .hotkeys {
    font-size: 10px;
    color: #8a7a5a;
    text-align: center;
    padding-top: 2px;
  }
  .ok { color: #00c88a; font-size: 11px; }
  .warn { color: #e0b040; font-size: 11px; }

  input[type='range'] {
    flex: 1;
    accent-color: #c3af75;
    height: 14px;
  }
  input[type='range']:disabled { opacity: 0.4; }

  .pct { width: 38px; text-align: right; flex: none; font-size: 12px; }

  .src {
    flex: 1;
    font-size: 11px;
    color: #8a7a5a;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .btn {
    box-sizing: border-box;
    height: 21px;
    width: 66px;
    flex: none;
    font: inherit;
    font-size: 11px;
    color: #e8d9b0;
    text-shadow: 0 1px 0 #1a0a0a;
    background: var(--btn) no-repeat;
    background-size: 100% 100%;
    image-rendering: pixelated;
    border: none;
    cursor: pointer;
    padding: 0 0 2px;
  }
  .btn:hover:not(:disabled) { background-image: var(--btn-hover); }
  .btn:active:not(:disabled) { background-image: var(--btn-down); }
  .btn:disabled { opacity: 0.4; cursor: default; }

  .close {
    position: absolute;
    top: -8px;
    right: -8px;
    width: 21px;
    height: 21px;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
    z-index: 1;
  }
  .close img { position: absolute; inset: 0; width: 21px; height: 21px; }
  .close .close-hover { display: none; }
  .close:hover .close-normal { display: none; }
  .close:hover .close-hover { display: block; }

  .c-ang { color: #f6f794; }
  .c-her { color: #00ffae; }
  .c-sat { color: #ca1717; }
  .c-set { color: #40d040; }
  .c-unh { color: #e04a7a; }
  .c-ble { color: #f0e8b0; }
  .c-myt { color: #c060e0; }
  .c-gold { color: #e8c860; }
</style>
