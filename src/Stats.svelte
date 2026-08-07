<script>
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { buffInfo, debuffInfo, zoneName } from './buffs.js';
  import { itemName, rarityByName, tierLabel, typeLabel } from './items.js';

  import panelBg from './assets/game/panel.png';
  import chipBg from './assets/game/chip_dark.png';
  import closeImg from './assets/game/close.png';
  import closeHoverImg from './assets/game/close_hover.png';
  import headerBg from './assets/game/header.png';

  let snap = $state(null);
  let extra = $state(null);
  let canvas;

  // pushed by the backend, and only while this window is on screen
  let clock = $state({ secs: 0, at: Date.now() });
  function received(s) {
    snap = s;
    clock = { secs: s.session_secs, at: Date.now() };
  }

  $effect(() => {
    invoke('snapshot').then(received).catch(() => {});
    invoke('get_extra')
      .then((e) => {
        extra = e;
        drawGraph();
      })
      .catch(() => {});
    const unsubs = [
      listen('stats', (e) => received(e.payload)),
      listen('stats-extra', (e) => {
        extra = e.payload;
        drawGraph();
      }),
    ];
    return () => unsubs.forEach((u) => u.then((f) => f()));
  });

  function fmt(n) {
    const v = n ?? 0;
    const abs = Math.abs(v);
    if (abs >= 1e9) return `${(v / 1e9).toFixed(2)}kkk`;
    if (abs >= 1e6) return `${(v / 1e6).toFixed(2)}kk`;
    if (abs >= 10_000) return `${(v / 1e3).toFixed(1)}k`;
    return v.toLocaleString('en-US');
  }

  function dur(secs) {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    return h > 0
      ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
      : `${m}:${String(s).padStart(2, '0')}`;
  }

  const time = (ms) =>
    new Date(ms).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' });

  const DIFFICULTIES = ['Normal', 'Nightmare', 'Hell'];

  const item = (name) => snap?.items?.[name] ?? { total: 0, mf: 0, per_hour: 0 };

  let charSub = $derived.by(() => {
    const c = extra?.character;
    if (!c) return 'waiting for character…';
    const parts = [];
    if (c.name) parts.push(c.name);
    parts.push(`Lv ${c.level}`, `HLv ${c.herolevel}`, DIFFICULTIES[c.difficulty] ?? `D${c.difficulty}`);
    if (c.hardcore) parts.push('HC');
    return parts.join(' · ');
  });

  // gold, xp and kills only travel when the game saves the character or banks
  // gold; a stale number is the game being quiet, not the tracker being stuck
  const ago = (secs) => (secs < 90 ? `${secs}s` : `${Math.floor(secs / 60)}m`);
  let lag = $derived.by(() => {
    const save = snap?.save_age_secs;
    const bank = snap?.bank_age_secs;
    const parts = [];
    if (save != null && save >= 45) parts.push(`character save ${ago(save)} ago`);
    if (bank != null && bank >= 45) parts.push(`balance ${ago(bank)} ago`);
    if (save == null && bank == null) return 'waiting for the first game save — gold, xp and kills arrive with it';
    return parts.length ? `last from the game · ${parts.join(' · ')}` : '';
  });

  let buffs = $derived((snap?.satanic_zone?.buffs ?? []).slice(0, 4).map(buffInfo));
  let debuffs = $derived((snap?.satanic_zone?.debuffs ?? []).slice(0, 4).map(debuffInfo));

  // zones rotate on the half hour (:00 / :30), aligned to the wall clock
  let nowTick = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (nowTick = Date.now()), 1000);
    return () => clearInterval(t);
  });
  let zoneReset = $derived.by(() => {
    const d = new Date(nowTick);
    const next = new Date(d);
    next.setMinutes(d.getMinutes() < 30 ? 30 : 60, 0, 0);
    return {
      at: next.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' }),
      in: dur(Math.max(0, Math.floor((next.getTime() - nowTick) / 1000))),
    };
  });

  function dropLabel(d) {
    if (d.name) return d.name;
    const known = itemName(d.item_type, d.item_id, d.weapon_type);
    if (known) return known;
    if (d.item_id > 0) return `${typeLabel(d.item_type, d.weapon_type)} #${d.item_id}`;
    const parts = [];
    if (d.item_type > 0) parts.push(typeLabel(d.item_type, d.weapon_type));
    if (d.seed > 0) parts.push(`Seed ${String(d.seed).slice(-6)}`);
    return parts.join(' · ') || 'Unknown item';
  }

  function dropRarity(d) {
    if (d.rarity) return d.rarity;
    const byName = rarityByName(dropLabel(d));
    return byName ?? 'Drop';
  }

  // rolling per-hour rates from the 15s cumulative series
  function rates() {
    const s = extra?.series ?? [];
    const out = [];
    const K = 4;
    for (let i = 1; i < s.length; i++) {
      const j = Math.max(0, i - K);
      const dt = s[i].t - s[j].t;
      if (dt <= 0) continue;
      out.push({
        t: s[i].t,
        gold: ((s[i].gold - s[j].gold) * 3600) / dt,
        xp: ((s[i].xp - s[j].xp) * 3600) / dt,
      });
    }
    return out;
  }

  function drawGraph() {
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    const W = canvas.width;
    const H = canvas.height;
    ctx.clearRect(0, 0, W, H);
    const data = rates();
    if (data.length < 2) {
      ctx.fillStyle = '#8a7a5a';
      ctx.font = '12px sans-serif';
      ctx.fillText('the graph appears after a couple of minutes of farming', 12, H / 2 + 4);
      return;
    }
    const t0 = data[0].t;
    const t1 = data[data.length - 1].t;
    const span = Math.max(1, t1 - t0);
    const maxGold = Math.max(...data.map((d) => d.gold), 1);
    const maxXp = Math.max(...data.map((d) => d.xp), 1);
    const px = (t) => ((t - t0) / span) * (W - 8) + 4;
    const line = (key, max, color) => {
      ctx.beginPath();
      data.forEach((d, i) => {
        const x = px(d.t);
        const y = H - 6 - (d[key] / max) * (H - 22);
        i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
      });
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.5;
      ctx.stroke();
    };
    line('gold', maxGold, '#e8c860');
    line('xp', maxXp, '#a06ae0');
    ctx.font = '11px sans-serif';
    ctx.fillStyle = '#e8c860';
    ctx.fillText(`gold/h peak ${fmt(Math.round(maxGold))}`, 8, 12);
    ctx.fillStyle = '#a06ae0';
    ctx.fillText(`xp/h peak ${fmt(Math.round(maxXp))}`, 170, 12);
  }

  const rarityCls = {
    Satanic: 'c-sat',
    Heroic: 'c-her',
    Angelic: 'c-ang',
    Unholy: 'c-unh',
    Mythic: 'c-myt',
    Set: 'c-set',
    Runeword: 'c-gold',
  };

  const hide = () => invoke('hide_stats');
</script>

<div class="panel" style:border-image-source="url({panelBg})" data-tauri-drag-region>
  <button class="close" onclick={hide} title="Close" aria-label="close">
    <img src={closeImg} alt="" class="close-normal" />
    <img src={closeHoverImg} alt="" class="close-hover" />
  </button>

  <div class="title" style:background-image="url({headerBg})" data-tauri-drag-region>
    <span>Statistics</span>
  </div>

  <div class="body">
  <div class="cards" data-tauri-drag-region>
    <div class="card" style:border-image-source="url({chipBg})">
      <div class="label">This session</div>
      <div class="value">{snap ? dur(clock.secs + (nowTick - clock.at) / 1000) : '0:00'}</div>
      <div class="sub">{charSub}</div>
    </div>
    <div class="card" style:border-image-source="url({chipBg})">
      <div class="label">Gold</div>
      <div class="value c-gold">{fmt(snap?.gold.earned)}</div>
      <div class="sub">{fmt(snap?.gold.per_hour)}/h · bank {fmt(snap?.gold.total)}</div>
    </div>
    <div class="card" style:border-image-source="url({chipBg})">
      <div class="label">XP</div>
      <div class="value c-xp">{fmt(snap?.xp.earned)}</div>
      <div class="sub">{fmt(snap?.xp.per_hour)}/h</div>
    </div>
    <div class="card" style:border-image-source="url({chipBg})">
      <div class="label">Kills</div>
      <div class="value c-her">{fmt(snap?.kills.earned)}</div>
      <div class="sub">{fmt(snap?.kills.per_hour)}/h · total {fmt(snap?.kills.total)}</div>
    </div>
  </div>

  <div class="cards four" data-tauri-drag-region>
    {#each [['Satanic', item('Satanic'), 'c-sat'], ['Set', item('Set'), 'c-set'], ['Heroic', item('Heroic'), 'c-her'], ['Angelic', item('Angelic'), 'c-ang'], ['Unholy', item('Unholy'), 'c-unh']] as [name, it, cls]}
      <div class="card" style:border-image-source="url({chipBg})">
        <div class="label">{name}</div>
        <div class="value {cls}">{fmt(it.total)}</div>
        <div class="sub">{fmt(it.mf)} MF · {fmt(it.per_hour)}/h</div>
      </div>
    {/each}
  </div>

  <div class="cards four" data-tauri-drag-region>
    {#each snap?.notable ?? [] as n}
      <div class="card" style:border-image-source="url({chipBg})" title={n.label}>
        <div class="label">{n.label}</div>
        <div class="value c-gold">{fmt(n.total)}</div>
      </div>
    {/each}
  </div>

  {#if lag}
    <div class="lag" data-tauri-drag-region>{lag}</div>
  {/if}

  <div class="resline" data-tauri-drag-region>
    <span>Keys <b>{fmt(snap?.resources?.keys)}</b></span>
    <span>Materials <b>{fmt(snap?.resources?.materials)}</b></span>
    <span>Socketables <b>{fmt(snap?.resources?.socketables)}</b></span>
    <span>Collectibles <b>{fmt(snap?.resources?.collectibles)}</b></span>
  </div>

  <div class="box" style:border-image-source="url({chipBg})">
    <div class="box-head">
      <span class="accent">Satanic Zone</span>
      <span class="right">resets in {zoneReset.in} · at {zoneReset.at}</span>
    </div>
    {#if snap?.satanic_zone}
      <div class="szname">{zoneName(snap.satanic_zone.zone)}</div>
      <div class="effects">
        <div class="effcol">
          <div class="effhead pros">Pros</div>
          {#each buffs as b}
            <div class="buffrow">
              <img src={b.icon} alt="" />
              <div>
                <div class="buffname">{b.name}</div>
                <div class="buffdesc" title={b.desc}>{b.desc}</div>
              </div>
            </div>
          {:else}
            <div class="buffdesc">—</div>
          {/each}
        </div>
        <div class="effcol">
          <div class="effhead cons">Cons</div>
          {#each debuffs as d}
            <div class="buffrow">
              <div>
                <div class="buffname cons">{d.name}</div>
                <div class="buffdesc" title={d.desc}>{d.desc}</div>
              </div>
            </div>
          {:else}
            <div class="buffdesc">—</div>
          {/each}
        </div>
      </div>
    {:else}
      <div class="sub center">no satanic zone data yet</div>
    {/if}
  </div>

  <div class="box" style:border-image-source="url({chipBg})">
    <div class="box-head"><span class="accent">Session rates</span></div>
    <canvas bind:this={canvas} width="506" height="84"></canvas>
  </div>

  <div class="box grow" style:border-image-source="url({chipBg})">
    <div class="box-head">
      <span class="accent">Item timeline</span>
      <span class="right">{extra?.drops?.length ?? 0} drops</span>
    </div>
    <div class="list">
      {#each extra?.drops ?? [] as d}
        <div class="drop">
          <span class="ts">{time(d.ts_ms)}</span>
          <span class="rar {rarityCls[dropRarity(d)] ?? ''}">{dropRarity(d)}</span>
          <span class="name {rarityCls[dropRarity(d)] ?? ''}" title={dropLabel(d)}>{dropLabel(d)}</span>
          <span class="dim tier">{tierLabel(d.tier)}</span>
          <span class="c-blue mf">{d.mf ? 'MF' : ''}</span>
          {#if d.announced}<span class="dim">server</span>{/if}
        </div>
      {:else}
        <div class="dim empty">nothing yet — valuable drops land here</div>
      {/each}
    </div>
  </div>
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

  :global(#app) { height: 100%; }
  :global(img) { image-rendering: pixelated; }

  .panel {
    position: relative;
    box-sizing: border-box;
    width: 560px;
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
    font-size: 12px;
    color: #c3af75;
  }

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

  .cards {
    display: flex;
    gap: 6px;
    flex: none;
  }

  .card {
    box-sizing: border-box;
    flex: 1;
    min-width: 0;
    border: 6px solid transparent;
    border-image-slice: 6 fill;
    border-image-width: 6px;
    image-rendering: pixelated;
    padding: 2px 8px 4px;
  }

  .label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: #9a8a68;
  }
  .value { font-size: 19px; line-height: 22px; }
  .cards.four .value { font-size: 15px; line-height: 18px; }
  .cards.four .card { padding: 2px 6px 3px; }
  .cards.four .sub { font-size: 9px; }
  .cards.four .label {
    font-size: 9px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .resline {
    flex: none;
    display: flex;
    gap: 18px;
    justify-content: center;
    font-size: 10px;
    color: #8a7a5a;
  }
  .resline b { color: #c3af75; font-weight: normal; }
  .sub {
    font-size: 10px;
    color: #8a7a5a;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .center { text-align: center; padding: 6px 0; }

  .box {
    box-sizing: border-box;
    flex: none;
    border: 6px solid transparent;
    border-image-slice: 6 fill;
    border-image-width: 6px;
    image-rendering: pixelated;
    padding: 4px 8px 6px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  /* the panel scrolls as a whole, so the timeline keeps a fixed frame */
  .box.grow { flex: none; height: 190px; }

  .lag {
    flex: none;
    font-size: 10px;
    color: #7b6a63;
    text-align: center;
    margin-top: -2px;
  }

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

  .box-head {
    flex: none;
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: #9a8a68;
    margin-bottom: 3px;
  }
  .accent { color: #ca4545; }
  .right { color: #8a7a5a; text-transform: none; letter-spacing: 0; }

  .szname { font-size: 15px; margin-bottom: 4px; }

  .effects {
    display: flex;
    gap: 12px;
  }
  .effcol {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .effhead {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }
  .effhead.pros { color: #00c88a; }
  .effhead.cons { color: #ca4545; }
  .buffrow {
    display: flex;
    gap: 8px;
    align-items: center;
    min-width: 0;
  }
  .buffrow img { width: 21px; height: 21px; flex: none; }
  .buffrow > div { min-width: 0; }
  .buffname { font-size: 12px; color: #e0cc90; line-height: 14px; }
  .buffname.cons { color: #d09090; }
  .buffdesc {
    font-size: 10px;
    color: #8a7a5a;
    line-height: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  canvas { display: block; flex: none; }

  .list {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .list::-webkit-scrollbar { width: 6px; }
  .list::-webkit-scrollbar-thumb { background: #4a3a3a; border-radius: 3px; }

  .drop {
    display: flex;
    gap: 8px;
    align-items: baseline;
    white-space: nowrap;
    flex: none;
  }
  .ts { color: #7a6a4e; font-size: 11px; width: 62px; flex: none; }
  .rar { width: 54px; flex: none; font-size: 11px; }
  .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .tier { width: 28px; flex: none; }
  .mf { width: 20px; flex: none; }
  .dim { color: #8a7a5a; font-size: 11px; }
  .zone { overflow: hidden; text-overflow: ellipsis; }
  .empty { padding: 8px 0; text-align: center; width: 100%; }

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
  .c-blue { color: #5050ae; }
  .c-myt { color: #c060e0; }
  .c-unh { color: #e04a7a; }
  .c-set { color: #40d040; }
  .c-ble { color: #f0e8b0; }
  .c-gold { color: #e8c860; }
  .c-xp { color: #a06ae0; }
</style>
