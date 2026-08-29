<script>
  // The window that stops the screen for a drop worth stopping it for.
  //
  // It is the game's own unique-loot pillar, its sparkle burst and the glow that
  // pools under a dropped item, tinted to the rarity. The sprites are white with
  // the shape in their alpha (tools/export_ui.py turns their brightness into it),
  // so they are used as masks and painted, which is what lets one set of frames
  // serve every rarity.
  import { appWindow, invoke, listen, native } from './bridge.js';
  import { itemName, tierLabel, typeLabel } from './items.js';
  import { statLabel } from './item-stats.js';
  import { normaliseFxProfile, STARTER_FX_PRESETS } from './fx-presets.js';
  import {
    FLOURISH_FAMILY_LABELS,
    flourishFamily,
    normalisePlacementEvent,
  } from './flourish-family.js';
  import { buffInfo, icon, zoneName } from './buffs.js';
  import { art } from './skin.svelte.js';

  const RARITY_TINT = {
    Satanic: '#ca1717',
    Set: '#40d040',
    Heroic: '#00ffae',
    Angelic: '#f6f794',
    Unholy: '#e04a7a',
  };
  /// How the entrance and the exit are timed. They are fixed lengths rather
  /// than a share of the run: stretched to a share, a longer setting would only
  /// make the thing fade in slowly, when what the player asked for is a longer
  /// look at it. The middle is what grows.
  const IN_MS = 320;
  const OUT_MS = 600;

  let drop = $state(null);
  let playing = $state(false);
  /// bumped for every play: the effect is keyed on it, so its nodes are built
  /// afresh and the animations start from nothing. Toggling a class instead
  /// looks right and does nothing — the browser coalesces off-and-on-again
  /// within a frame into no change at all.
  let run = $state(0);
  let placing = $state(false);
  let placingFamily = $state('loot');
  let placementError = $state('');
  let positioning = $state(false);
  let cfg = $state(null);
  let timer = null;
  let advancing = false;
  let queueEpoch = 0;

  /// Drops waiting their turn. A boss can hand over three things at once, and
  /// announcing them on top of one another means seeing none of them — so they
  /// queue, and the window stays up until the last has been shown.
  let waiting = [];
  let placementBuffer = [];
  let activeEntry = null;
  const QUEUE_CAP = 6;

  const SAMPLE = { rarity: 'Heroic', name: "Fenrir's Bloodfang", tier: 6, item_type: 3, weapon_type: 1 };
  const HIGH_ROLL_SAMPLE = {
    rarity: 'Set', name: "Gladiator's Skullet", tier: 4,
    item_type: 0, weapon_type: 0, high_roll: true, roll_percent: 78,
  };
  const STAT_SAMPLE = {
    rarity: 'Satanic', name: 'Rift Vectors', tier: 6,
    item_type: 6, weapon_type: 0,
    stat_matches: [{ stat_id: 70, stat: 'projectile_speed', actual: 3, op: '>', target: 2 }],
  };
  /// The other thing this window draws, so a position can be judged against
  /// both before it is committed to. The zone announcement is the wider of the
  /// two, and picking a spot that only suits the drop is how it ends up half
  /// off the screen an hour later.
  const ZONE_SAMPLE = {
    kind: 'zone',
    zone: 'Satanic_5_2',
    buffs: [2, 3, 14, 21, 24],
    debuffs: [3, 9],
    colossal_chest: true,
  };
  const TWITCH_SAMPLE = {
    kind: 'twitch',
    event: 'raid',
    actor: 'SiegeRaider',
    headline: 'Incoming raid',
    detail: '128 viewers',
    viewers: 128,
  };
  const PLACEMENT_SAMPLES = {
    loot: SAMPLE,
    high_roll: HIGH_ROLL_SAMPLE,
    stat: STAT_SAMPLE,
    zone: ZONE_SAMPLE,
    twitch: TWITCH_SAMPLE,
  };
  /// More than this and the plate is a wall of names nobody reads inside six
  /// seconds; the rest are counted instead.
  const BUFFS_SHOWN = 5;
  const TWITCH_HEADINGS = {
    follow: 'New follower', new_sub: 'New subscriber', resub: 'Resubscription',
    sub_gift: 'Gift subscriptions', bits: 'Bits', power_up: 'Power-up', raid: 'Incoming raid',
    channel_points: 'Channel Points', automatic_points: 'Channel Points', charity_donation: 'Charity donation',
    hype_train: 'Hype Train', goal: 'Creator goal', poll: 'Poll', prediction: 'Prediction',
    shoutout: 'Shoutout', stream_online: 'Stream online', stream_offline: 'Stream offline',
    ad_break: 'Ad break', channel_update: 'Channel updated', chat_announcement: 'Announcement',
    watch_streak: 'Watch streak', modiversary: 'Modiversary', bits_badge: 'Bits badge', user_intro: 'First message',
  };

  function errorMessage(error) {
    if (typeof error === 'string') return error;
    if (error && typeof error.message === 'string') return error.message;
    return '';
  }

  async function finishPlacement(args, verb) {
    placementError = '';
    try {
      await invoke('place_flourish', args);
    } catch (error) {
      const detail = errorMessage(error);
      placementError = `Could not ${verb}${detail ? `: ${detail}` : '.'}`;
    }
  }

  const stopPlacing = () => finishPlacement({ placing: false }, 'save this location');
  const applyPlacementToAll = () => finishPlacement(
    { placing: false, applyAll: true },
    'use this location for every alert',
  );
  const cancelPlacing = () => finishPlacement({ placing: false, cancel: true }, 'cancel placement');
  const placementSample = (family) => PLACEMENT_SAMPLES[family] ?? SAMPLE;

  function alertPriority(entry) {
    if (entry?.kind === 'twitch') {
      if (Number.isFinite(Number(entry.priority))) return Number(entry.priority);
      if (['raid', 'sub_gift', 'charity_donation', 'hype_train'].includes(entry.event)) return 4;
      if (['new_sub', 'resub', 'bits', 'power_up'].includes(entry.event)) return 3;
      return 1;
    }
    if (entry?.kind === 'zone') return 3;
    if ((entry?.stat_matches?.length ?? 0) > 0) return 2;
    if (entry?.high_roll) return 1;
    return 0;
  }

  function numberLabel(value) {
    const number = Number(value);
    if (!Number.isFinite(number)) return String(value ?? '');
    return Number.isInteger(number) ? String(number) : String(Number(number.toFixed(4)));
  }

  function particleStyle(index, count, speed = 1) {
    // Deterministic scatter: rebuilding the keyed alert gives the particles a
    // fresh animation without letting Math.random make visual tests flaky.
    const x = (index * 37 + 11) % 100;
    const drift = ((index * 29) % 54) - 27;
    const delay = -((index * 113) % 1200);
    const duration = Math.round((1200 + ((index * 173) % 1000)) / speed);
    return `--px:${x}%;--drift:${drift}px;--delay:${delay}ms;--life:${duration}ms;--order:${index / Math.max(1, count)}`;
  }

  // While it is being placed this window takes the mouse, and a window that
  // takes the mouse and cannot be dismissed is a trap: it sits over whatever is
  // underneath and swallows every click meant for it. Escape always ends it.
  $effect(() => {
    const key = (e) => {
      if (e.key === 'Escape' && placing) cancelPlacing();
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  });

  $effect(() => {
    invoke('get_settings').then((s) => (cfg = s)).catch(() => {});
    const unsubs = [
      listen('settings-changed', (e) => (cfg = e.payload)),
      listen('flourish-play', (e) => {
        if (placing) bufferLive(e.payload);
        else enqueue(e.payload);
      }),
      listen('flourish-placing', (e) => {
        const request = normalisePlacementEvent(e.payload);
        const wasPlacing = placing;
        placementError = '';

        // Entering placement interrupts the screen immediately, but does not
        // throw away a live alert already selected or the queue behind it.
        if (request.placing && !wasPlacing) {
          if (activeEntry) bufferLive(activeEntry);
          for (const entry of waiting) bufferLive(entry);
        }
        queueEpoch += 1;
        clearTimeout(timer);
        timer = null;
        waiting.length = 0;
        playing = false;
        positioning = false;
        drop = null;
        activeEntry = null;
        placing = request.placing;
        if (placing) {
          placingFamily = request.family;
          // One family, one preview. Entering placement cancels whatever was
          // playing and immediately starts the selected sample.
          enqueue(placementSample(placingFamily));
        } else if (placementBuffer.length) {
          // Save, apply-all and cancel all end by removing the preview first,
          // then returning the live alerts to normal family-aware playback.
          waiting.push(...placementBuffer);
          placementBuffer.length = 0;
          void advance();
        }
      }),
    ];
    // The native handle is visible before these promises have installed their
    // listeners. Tell preview commands when an event can no longer be lost in
    // that small startup gap.
    Promise.all(unsubs).then(() => invoke('flourish_ready')).catch(() => {});
    return () => {
      queueEpoch += 1;
      clearTimeout(timer);
      unsubs.forEach((u) => u.then((f) => f()));
    };
  });

  function enqueue(entry) {
    if (!entry) return;
    if (!pushBounded(waiting, entry)) return;
    if (!playing && !advancing) void advance();
  }

  function bufferLive(entry) {
    if (!entry) return;
    pushBounded(placementBuffer, entry);
  }

  function pushBounded(queue, entry) {
    // a stack of pending announcements longer than this is a wall of light
    // nobody reads; the counters have them all either way
    if (queue.length >= QUEUE_CAP) {
      // Only a genuinely more important alert can displace something already
      // queued: rotation > custom stat > high roll > ordinary drop.
      let lowest = 0;
      for (let i = 1; i < queue.length; i += 1) {
        if (alertPriority(queue[i]) < alertPriority(queue[lowest])) lowest = i;
      }
      if (alertPriority(entry) <= alertPriority(queue[lowest])) return false;
      queue.splice(lowest, 1);
    }
    queue.push(entry);
    return true;
  }

  async function advance() {
    if (advancing) return;
    advancing = true;
    clearTimeout(timer);
    timer = null;
    const epoch = queueEpoch;
    const next = waiting.shift();
    if (!next) {
      activeEntry = null;
      playing = false;
      positioning = false;
      drop = null;
      // Keep hide in the same serial lane as positioning. If a live event is
      // enqueued while this IPC is in flight, it waits here and is positioned
      // (which also shows the native window) only after the old hide finishes.
      // A fire-and-forget hide could otherwise overtake the next alert and make
      // it disappear part-way through its animation.
      if (!placing && native) {
        try {
          await invoke('flourish_done');
        } catch {
          // A failed hide is cosmetic; the next queue entry must still run.
        }
      }
      advancing = false;
      if (!playing && waiting.length) void advance();
      return;
    }
    // `playing` is also the queue lock. Set it before the awaited move so an
    // event arriving during IPC cannot start a second advance in parallel.
    playing = true;
    positioning = true;
    drop = null;
    activeEntry = next;
    const family = flourishFamily(next);
    try {
      // Moving a shared transparent window after rendering causes the alert to
      // flash at the previous family's location. The queue does not expose a
      // live payload until its position has been applied. Placement itself has
      // already moved the window before emitting flourish-placing; moving on
      // every preview loop would undo the drag and snap back to the saved point.
      if (!placing) await invoke('position_flourish', { family });
      if (epoch !== queueEpoch) return;
      drop = next;
      positioning = false;
      run += 1;
      timer = setTimeout(() => {
        timer = null;
        if (placing) waiting.push(placementSample(placingFamily));
        void advance();
      }, runMs);
    } catch {
      if (epoch !== queueEpoch) return;
      // Position failure must not jam the alert queue; render where the window
      // already is and let the next event make another attempt.
      drop = next;
      positioning = false;
      run += 1;
      timer = setTimeout(() => {
        timer = null;
        if (placing) waiting.push(placementSample(placingFamily));
        void advance();
      }, runMs);
    } finally {
      advancing = false;
      // A placement-family change can invalidate the awaited move and queue a
      // replacement sample while this advance still owns the lock.
      if (epoch !== queueEpoch && !playing && waiting.length) void advance();
      if (epoch !== queueEpoch && !placing && !waiting.length) {
        playing = false;
        positioning = false;
        drop = null;
      }
    }
  }

  let animating = $derived(Boolean(playing && !positioning && drop));
  let placingLabel = $derived(FLOURISH_FAMILY_LABELS[placingFamily] ?? FLOURISH_FAMILY_LABELS.loot);
  let debugFx = $derived(Boolean(cfg?.debug_mode));

  let isZone = $derived(drop?.kind === 'zone');
  let isTwitch = $derived(drop?.kind === 'twitch');
  let isColossalZone = $derived(Boolean(isZone && drop?.colossal_chest));
  let isHighRoll = $derived(Boolean(!isZone && drop?.high_roll));
  let isStatAlert = $derived(Boolean(!isZone && (drop?.stat_matches?.length ?? 0) > 0));
  /// What the plate lists, capped, with the overflow kept as a count rather
  /// than dropped silently.
  let zbuffs = $derived.by(() => {
    const ids = drop?.buffs ?? [];
    const shown = ids.slice(0, BUFFS_SHOWN).map((id) => ({ id, ...buffInfo(id) }));
    if (ids.length > BUFFS_SHOWN) shown.push({ id: -1, more: ids.length - BUFFS_SHOWN });
    return shown;
  });

  let profile = $derived.by(() => {
    // FX Lab is an experimental Debug Mode feature. Outside it, payload and
    // saved presets must not quietly keep those effects live; only the three
    // long-standing alert controls seed the classic renderer.
    if (!debugFx) {
      return normaliseFxProfile({
        duration_s: cfg?.flourish_secs,
        scale: cfg?.flourish_scale,
        shade: cfg?.flourish_shade,
        quality_escalation: false,
        stat_fx_enabled: false,
      });
    }
    const presetId = String(drop?.fx_preset ?? '');
    const preset = STARTER_FX_PRESETS.find((entry) => entry.id === presetId)
      ?? (cfg?.flourish_fx_presets ?? []).find((entry) => entry?.id === presetId);
    const configured = cfg?.flourish_fx;
    const savedProfile = configured
      && typeof configured === 'object'
      && !Array.isArray(configured)
      && Object.keys(configured).length > 0
      ? configured
      : null;
    return normaliseFxProfile(drop?.fx ?? preset?.fx ?? savedProfile ?? {
      duration_s: cfg?.flourish_secs,
      scale: cfg?.flourish_scale,
      shade: cfg?.flourish_shade,
    });
  });
  let rollPercent = $derived(Number(drop?.roll_percent ?? 0));
  let qualityBand = $derived.by(() => {
    if (!debugFx || !isHighRoll || !profile.quality_escalation) return isHighRoll ? 'high' : 'ordinary';
    if (rollPercent >= profile.quality_perfect) return 'perfect';
    if (rollPercent >= profile.quality_near) return 'near';
    if (rollPercent >= profile.quality_epic) return 'epic';
    return 'high';
  });
  let tint = $derived.by(() => {
    if (isTwitch) {
      // Twitch alerts do not have loot-roll quality, but they do have useful
      // visual roles. Map those roles into the selected preset's palette so a
      // Frost/Demonic/custom preset actually recolors the alert instead of the
      // backend's fixed Twitch tint winning every time.
      const event = String(drop?.event ?? '');
      if (['raid', 'sub_gift', 'charity_donation', 'hype_train'].includes(event)) return profile.colors.perfect;
      if (['new_sub', 'resub', 'bits', 'power_up'].includes(event)) return profile.colors.near_perfect;
      if (['channel_points', 'automatic_points', 'chat_announcement', 'watch_streak', 'modiversary', 'bits_badge', 'user_intro'].includes(event)) return profile.colors.stat;
      if (['stream_online', 'stream_offline', 'ad_break', 'channel_update'].includes(event)) return profile.colors.ordinary;
      return profile.colors.high_roll ?? drop?.color ?? '#a970ff';
    }
    if (isHighRoll && isStatAlert) return profile.colors.combined;
    if (isStatAlert) return profile.colors.stat;
    if (qualityBand === 'perfect') return profile.colors.perfect;
    if (qualityBand === 'near' || qualityBand === 'epic') return profile.colors.near_perfect;
    if (isHighRoll) return profile.colors.high_roll;
    // Preserve rarity identity for legacy settings. Once a profile exists, its
    // ordinary color is an intentional choice in the lab.
    if (cfg?.debug_mode && cfg?.flourish_fx && Object.keys(cfg.flourish_fx).length) return profile.colors.ordinary;
    return RARITY_TINT[drop?.rarity] ?? profile.colors.ordinary;
  });
  let statAccents = $derived.by(() => {
    if (!debugFx || !isStatAlert || !profile.stat_fx_enabled) return new Set();
    const found = new Set();
    for (const match of drop?.stat_matches ?? []) {
      const name = `${match?.stat ?? ''} ${statLabel(match?.stat_id)}`.toLowerCase();
      if (profile.projectile_trails && name.includes('projectile')) found.add('projectile');
      if (profile.vitality_pulse && (name.includes('vitality') || name.includes('life'))) found.add('vitality');
      if (profile.crushing_shockwave && name.includes('crushing')) found.add('crushing');
      if (profile.socket_orbit && name.includes('socket')) found.add('socket');
    }
    return found;
  });
  let particleCount = $derived(debugFx && profile.particles_enabled ? Math.round(profile.particle_density * 0.28) : 0);
  let burstPower = $derived(
    !debugFx ? 1
      : qualityBand === 'perfect' ? 1.5
      : qualityBand === 'near' ? 1.3
      : qualityBand === 'epic' ? 1.15
      : 1,
  );
  let alertHeading = $derived.by(() => {
    const percent = drop?.roll_percent ?? cfg?.high_roll_threshold ?? 75;
    if (isHighRoll && isStatAlert) return `Stat Alert + High Roll ${percent}%+ · ${drop.rarity}`;
    if (isStatAlert) return `Stat Alert · ${drop.rarity}`;
    if (isHighRoll) return `High Roll ${percent}%+ · ${drop.rarity}`;
    return drop?.rarity ?? '';
  });
  let statSummary = $derived.by(() => {
    const matches = drop?.stat_matches ?? [];
    if (!matches.length) return '';
    const first = matches[0];
    const symbol = { eq: '=', gt: '>', lt: '<' }[first.op] ?? first.op;
    const rest = matches.length > 1 ? `  ·  +${matches.length - 1} more` : '';
    return `${statLabel(first.stat_id)}  ${numberLabel(first.actual)} ${symbol} ${numberLabel(first.target)}${rest}`;
  });
  let twitchHeading = $derived(drop?.headline ?? TWITCH_HEADINGS[drop?.event] ?? 'Twitch alert');
  let twitchActor = $derived(drop?.actor ?? drop?.user_name ?? 'Twitch');
  let twitchDetail = $derived.by(() => {
    if (drop?.detail) return drop.detail;
    if (drop?.viewers) return `${numberLabel(drop.viewers)} viewers`;
    if (drop?.count) return `${numberLabel(drop.count)} gifted`;
    if (drop?.amount && drop?.currency) return `${numberLabel(drop.amount)} ${drop.currency}`;
    if (drop?.amount) return numberLabel(drop.amount);
    return '';
  });
  let twitchMessage = $derived.by(() => {
    const message = String(drop?.message ?? '').trim();
    if (!message) return '';
    // The backend-rendered template is `detail`. If the template used
    // {message}, printing the raw message as a second line says the same thing
    // twice; keep the second line only when it contributes new information.
    const detail = String(drop?.detail ?? '').toLocaleLowerCase();
    return detail.includes(message.toLocaleLowerCase()) ? '' : message;
  });
  let label = $derived.by(() => {
    if (!drop || drop.kind === 'zone') return '';
    if (drop.name) return drop.name;
    const known = itemName(drop.item_type, drop.item_id, drop.weapon_type);
    return known ?? typeLabel(drop.item_type, drop.weapon_type);
  });
  // Preserve the original public controls and their original bounds outside
  // Debug Mode. FX profile normalisation intentionally uses a narrower shade
  // range, which must not subtly alter the long-standing renderer.
  let runMs = $derived(Math.round((debugFx
    ? profile.duration_s
    : Math.min(12, Math.max(2, cfg?.flourish_secs ?? 6))) * 1000));
  let scale = $derived(debugFx
    ? profile.scale
    : Math.min(2, Math.max(0.5, cfg?.flourish_scale ?? 1)));
  let shade = $derived(debugFx
    ? profile.shade
    : Math.min(1, Math.max(0, cfg?.flourish_shade ?? 0.55)));
</script>

<div
  class={debugFx
    ? `stage layout-${profile.layout} edge-${profile.edge_position} entrance-${profile.entrance} quality-${qualityBand}`
    : 'stage'}
  class:playing={animating}
  class:placing
  class:reduced={debugFx && profile.reduce_motion}
  style:--tint={tint}
  style:--scale={scale}
  style:--in="{IN_MS}ms"
  style:--out="{OUT_MS}ms"
  style:--hold="{Math.max(0, runMs - OUT_MS)}ms"
  style:--shade={shade}
  style:--font-scale={debugFx ? profile.font_scale : 1}
  style:--glow-power={debugFx ? profile.glow_intensity : 1}
  style:--particle-size={debugFx ? profile.particle_size : 1}
  style:--particle-speed={debugFx ? profile.particle_speed : 1}
  style:--flash={debugFx && !profile.reduce_motion ? profile.screen_flash : 0}
  style:--edge-inset={debugFx ? `${profile.edge_inset}px` : '0px'}
  style:--burst={debugFx ? burstPower : 1}
  style:--sparks="url({art('fx_sparks')})"
  style:--glow="url({art('fx_glow')})"
  style:--plate="url({art('header')})"
  style:--chipart="url({art('chip_dark')})"
>
  {#key run}
    {#if isZone}
      <!-- A drop is a column of light standing on a pool; a rotation is a band
           that splits open across the window. Not one node in common with the
           branch below, which is what makes them impossible to confuse from the
           other side of the room — the point of the window. -->
      <div class="zfx" class:playing={animating} class:colossal={isColossalZone}>
        <div class="rift">
          <div class="band"><div class="sweep"></div></div>
          <div class="zbody">
            <div class="zkind">
              <img src={isColossalZone ? icon('chest') : art('satanic_star')} alt="" />
              <span class="txt">{isColossalZone ? 'Colossal Chest Zone' : 'Satanic Zone'}</span>
              <img src={isColossalZone ? icon('chest') : art('satanic_star')} alt="" />
              {#if drop.debuffs?.length}
                <!-- the buffs are the decision, the curses are the small print:
                     spelling them out doubles the height for something nobody
                     picks a zone by -->
                <span class="zcurse">{drop.debuffs.length} curses</span>
              {/if}
            </div>
            <div class="zplate"><span class="zname">{zoneName(drop.zone)}</span></div>
            {#if zbuffs.length}
              <div class="zbuffs">
                <!-- keyed by position: the ids come from the packet and
                     nothing dedupes them, and a repeat would throw
                     `each_key_duplicate` in a window nobody can see fail -->
                {#each zbuffs as b, i (i)}
                  <div class="zbuff" class:more={b.more}>
                    {#if b.more}
                      <span class="bname">+{b.more} more</span>
                    {:else}
                      <img src={b.icon} alt="" />
                      <span class="bname">{b.name}</span>
                    {/if}
                  </div>
                {/each}
              </div>
            {:else}
              <div class="znone">no buffs this rotation</div>
            {/if}
          </div>
        </div>
      </div>
    {:else if isTwitch}
      <div class="twfx" class:playing={animating}>
        <div class="twshade"></div>
        {#if debugFx && profile.screen_flash > 0 && !profile.reduce_motion}<div class="flash"></div>{/if}
        {#if debugFx && profile.beam_enabled}<div class="beam"></div>{/if}
        {#if debugFx && profile.shockwave_enabled}<div class="shockwave"></div>{/if}
        {#if !debugFx || profile.glow_enabled}<div class="glow"></div>{/if}
        {#if !debugFx || profile.particles_enabled}
          {#if debugFx}
            <div class="particle-field" class:trails={profile.particle_trails}>
              {#each Array(particleCount) as _, i}<i style={particleStyle(i, particleCount, profile.particle_speed)}></i>{/each}
            </div>
          {/if}
          <div class="sparks left"></div>
          <div class="sparks right"></div>
          <div class="sparks over"></div>
        {/if}
        <div class="twframe">
          <div class="twmark"><span>LIVE</span></div>
          <div class="twcopy">
            {#if !debugFx || profile.show_heading}<span class="twheading">{twitchHeading}</span>{/if}
            {#if !debugFx || profile.show_item_name}<span class="twactor">{twitchActor}</span>{/if}
            {#if (!debugFx || profile.show_tier) && twitchDetail}<span class="twdetail">{twitchDetail}</span>{/if}
            {#if (!debugFx || profile.show_stat) && twitchMessage}<span class="twmessage">{twitchMessage}</span>{/if}
          </div>
          <div class="twpulse"></div>
        </div>
      </div>
    {:else}
    <div class="fx" class:playing={animating}>
      <!-- The shading is a pool of shadow rather than a panel: the window is
           transparent, so a solid background would be a black box sitting on
           the game. It darkens the middle and fades to nothing at the edges. -->
      <div class="shade"></div>
      {#if debugFx && profile.screen_flash > 0 && !profile.reduce_motion}<div class="flash"></div>{/if}
      {#if debugFx && profile.beam_enabled}<div class="beam"></div>{/if}
      {#if debugFx && (profile.shockwave_enabled || statAccents.has('crushing'))}<div class="shockwave"></div>{/if}
      {#if !debugFx || profile.glow_enabled}<div class="glow"></div>{/if}
      {#if !debugFx || profile.particles_enabled}
        {#if debugFx}
          <div class="particle-field" class:trails={profile.particle_trails}>
            {#each Array(particleCount) as _, i}<i style={particleStyle(i, particleCount, profile.particle_speed)}></i>{/each}
          </div>
        {/if}
        <div class="sparks left"></div>
        <div class="sparks right"></div>
        <div class="sparks over"></div>
      {/if}
      {#if debugFx && statAccents.has('projectile')}
        <div class="projectile-streaks">{#each Array(7) as _, i}<i style={`--i:${i}`}></i>{/each}</div>
      {/if}
      {#if debugFx && statAccents.has('vitality')}<div class="vitality-pulse">♥</div>{/if}
      {#if debugFx && statAccents.has('socket')}
        <div class="socket-orbit">{#each Array(4) as _, i}<i style={`--i:${i}`}></i>{/each}</div>
      {/if}
      {#if drop}
        <div class="caption">
          {#if !debugFx || profile.show_heading}<span class="rar">
            {alertHeading}
          </span>{/if}
          {#if !debugFx || profile.show_item_name}<span class="name">{label}</span>{/if}
          {#if (!debugFx || profile.show_tier) && drop.tier > 0}<span class="grade">{tierLabel(drop.tier)}</span>{/if}
          {#if (!debugFx || profile.show_stat) && statSummary}<span class="statline">{statSummary}</span>{/if}
        </div>
      {/if}
    </div>
    {/if}
  {/key}

  {#if placing}
    <!-- while it is being placed the window takes the mouse, so it can be
         dragged, and says what it is -->
    <div class="place" data-tauri-drag-region>
      <div class="placing-kind" data-tauri-drag-region>Placing · {placingLabel}</div>
      <div class="hint" data-tauri-drag-region>Drag this box to the location for this alert family</div>
      <div class="place-actions">
        <button class="done" onclick={stopPlacing}>Save this location</button>
        <button class="done all" onclick={applyPlacementToAll}>Use for all alerts</button>
        <button class="done cancel" onclick={cancelPlacing}>Cancel</button>
      </div>
      {#if placementError}<div class="place-error" role="alert">{placementError}</div>{/if}
      <div class="escape" data-tauri-drag-region>Esc cancels without saving</div>
    </div>
  {/if}
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
  }

  .stage {
    position: relative;
    width: 100vw;
    height: 100vh;
    font-family: 'CookieRun Bold', sans-serif;
    overflow: hidden;
  }

  /* FX Lab layout modes operate on the same transparent window. Compact keeps
     the composition centered but trims its visual footprint; edge mode pins
     it without changing the native window placement the player already chose. */
  .layout-compact .fx,
  .layout-compact .twfx { transform: scale(calc(var(--scale) * 0.82)); }
  .layout-compact .glow { width: 270px; height: 96px; }
  .layout-compact .sparks.left { margin-right: 165px; }
  .layout-compact .sparks.right { margin-left: 165px; }
  .layout-edge.edge-top .fx,
  .layout-edge.edge-top .twfx { align-items: flex-start; padding-top: var(--edge-inset); }
  .layout-edge.edge-bottom .fx,
  .layout-edge.edge-bottom .twfx { align-items: flex-end; padding-bottom: var(--edge-inset); }
  .layout-edge.edge-left .fx,
  .layout-edge.edge-left .twfx { justify-content: flex-start; padding-left: var(--edge-inset); }
  .layout-edge.edge-right .fx,
  .layout-edge.edge-right .twfx { justify-content: flex-end; padding-right: var(--edge-inset); }
  .layout-edge .fx,
  .layout-edge .twfx { box-sizing: border-box; }

  .flash,
  .beam,
  .shockwave,
  .particle-field,
  .projectile-streaks,
  .vitality-pulse,
  .socket-orbit {
    position: absolute;
    pointer-events: none;
  }
  .flash {
    inset: 0;
    opacity: 0;
    background: radial-gradient(ellipse 36% 34% at 50% 50%, color-mix(in srgb, var(--tint) 78%, white), transparent 72%);
    mix-blend-mode: screen;
  }
  .fx.playing .flash,
  .twfx.playing .flash { animation: fx-flash 430ms ease-out forwards; }
  @keyframes fx-flash {
    0% { opacity: 0 }
    18% { opacity: var(--flash) }
    100% { opacity: 0 }
  }
  .beam {
    width: 76px;
    height: 390px;
    /* Absolute flex children keep a centred static position. Move the beam by
       half its own height so its base, rather than its middle, meets the item. */
    translate: 0 -50%;
    opacity: 0;
    transform-origin: 50% 100%;
    border-radius: 50%;
    background: linear-gradient(90deg, transparent, color-mix(in srgb, var(--tint) 55%, transparent), #fff 50%, color-mix(in srgb, var(--tint) 55%, transparent), transparent);
    filter: blur(4px) drop-shadow(0 0 18px var(--tint));
    -webkit-mask-image: linear-gradient(to bottom, transparent 0%, #000 22%, #000 86%, transparent 100%);
    mask-image: linear-gradient(to bottom, transparent 0%, #000 22%, #000 86%, transparent 100%);
  }
  .fx.playing .beam,
  .twfx.playing .beam {
    animation: beam-in var(--in) cubic-bezier(.16, 1, .3, 1) forwards,
               vanish var(--out) ease-in var(--hold) forwards;
  }
  @keyframes beam-in {
    from { opacity: 0; transform: scaleX(0.18) scaleY(0.1) }
    to { opacity: calc(0.46 * var(--burst)); transform: scaleX(var(--burst)) scaleY(1) }
  }
  .shockwave {
    width: 180px;
    height: 52px;
    border: 2px solid var(--tint);
    border-radius: 50%;
    opacity: 0;
    box-shadow: 0 0 12px var(--tint), inset 0 0 12px var(--tint);
  }
  .fx.playing .shockwave,
  .twfx.playing .shockwave { animation: shock 720ms cubic-bezier(.1,.7,.1,1) 120ms forwards; }
  @keyframes shock {
    from { opacity: calc(0.85 * var(--burst)); transform: scale(.2) }
    to { opacity: 0; transform: scale(calc(2.5 * var(--burst))) }
  }

  .particle-field {
    width: 420px;
    height: 190px;
    overflow: hidden;
    -webkit-mask-image: radial-gradient(ellipse, #000 48%, transparent 78%);
    mask-image: radial-gradient(ellipse, #000 48%, transparent 78%);
  }
  .particle-field i {
    position: absolute;
    left: var(--px);
    bottom: 12%;
    width: calc(2px * var(--particle-size));
    height: calc(2px * var(--particle-size));
    border-radius: 50%;
    background: color-mix(in srgb, var(--tint) 72%, white);
    box-shadow: 0 0 calc(6px * var(--particle-size)) var(--tint);
    opacity: 0;
  }
  .particle-field.trails i {
    height: calc(9px * var(--particle-size));
    border-radius: 60% 60% 20% 20%;
  }
  .fx.playing .particle-field i,
  .twfx.playing .particle-field i {
    animation: particle-rise var(--life) ease-out var(--delay) infinite;
  }
  @keyframes particle-rise {
    0% { opacity: 0; transform: translate(0, 16px) scale(.45) }
    18% { opacity: calc(.38 + var(--order) * .55) }
    100% { opacity: 0; transform: translate(var(--drift), -170px) scale(1.25) }
  }

  .projectile-streaks { width: 430px; height: 130px; }
  .projectile-streaks i {
    position: absolute;
    top: calc(12% + var(--i) * 12%);
    left: -25%;
    width: 95px;
    height: 2px;
    opacity: 0;
    background: linear-gradient(90deg, transparent, var(--tint), white);
    filter: drop-shadow(0 0 5px var(--tint));
  }
  .fx.playing .projectile-streaks i {
    animation: projectile 820ms ease-out calc(var(--i) * 65ms) forwards;
  }
  @keyframes projectile {
    from { opacity: 0; transform: translateX(0) skewX(-25deg) }
    20% { opacity: .9 }
    to { opacity: 0; transform: translateX(570px) skewX(-25deg) }
  }
  .vitality-pulse {
    color: var(--tint);
    font-size: 28px;
    opacity: 0;
    text-shadow: 0 0 14px var(--tint);
  }
  .fx.playing .vitality-pulse { animation: heartbeat 1.15s ease-in-out 300ms 2; }
  @keyframes heartbeat {
    0%, 100% { opacity: 0; transform: scale(.75) }
    18%, 42% { opacity: .9; transform: scale(1.2) }
    30%, 55% { opacity: .5; transform: scale(.92) }
  }
  .socket-orbit { width: 180px; height: 180px; animation: orbit-spin 3s linear infinite; }
  .socket-orbit i {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 8px;
    height: 8px;
    margin: -4px;
    border: 1px solid #fff;
    border-radius: 50%;
    background: var(--tint);
    box-shadow: 0 0 10px var(--tint);
    transform: rotate(calc(var(--i) * 90deg)) translateX(82px);
  }
  @keyframes orbit-spin { to { transform: rotate(360deg) } }

  .shade {
    position: absolute;
    inset: 0;
    background: radial-gradient(
      ellipse 46% 52% at 50% 50%,
      rgba(0, 0, 0, var(--shade)) 0%,
      rgba(0, 0, 0, calc(var(--shade) * 0.55)) 45%,
      rgba(0, 0, 0, 0) 72%
    );
    opacity: 0;
  }
  .fx.playing .shade {
    animation: appear var(--in) ease-out forwards,
               vanish var(--out) ease-in var(--hold) forwards;
  }
  @keyframes appear { from { opacity: 0 } to { opacity: 1 } }
  /* See main.js. Fading the shade in means twenty paints of a half-transparent
     black, and on a desktop that never clears the surface those add up: the
     soft pool arrives as a hard blob. There it is simply there, and simply
     gone — two paints, and the gradient keeps the shape it was given. */
  :global(html[data-os='linux']) .fx.playing .shade {
    animation: none;
    opacity: 1;
  }
  :global(html[data-os='linux']) .fx.playing .glow,
  :global(html[data-os='linux']) .twfx.playing .glow {
    animation: swell var(--in) ease-out forwards,
               vanish var(--out) ease-in var(--hold) forwards,
               glowframes 1s steps(15) infinite;
  }
  @keyframes vanish { from { opacity: 1 } to { opacity: 0 } }

  /* Everything centres on the name: the glow behind it, the sparks around it.
     The whole group scales together from its middle. */
  .fx {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    transform: scale(var(--scale));
  }

  .sparks, .glow {
    position: absolute;
    opacity: 0;
    background: var(--tint);
    -webkit-mask-repeat: no-repeat;
    mask-repeat: no-repeat;
    pointer-events: none;
  }

  /* the pool of light the game puts under a dropped item, stretched wide enough
     to sit behind a name rather than under a sprite */
  .glow {
    width: 340px;
    height: 120px;
    -webkit-mask-image: var(--glow);
    mask-image: var(--glow);
    -webkit-mask-size: 5100px 120px;
    mask-size: 5100px 120px;
  }
  .fx.playing .glow,
  .twfx.playing .glow {
    animation: swell var(--in) ease-out forwards,
               vanish var(--out) ease-in var(--hold) forwards,
               glowframes 1s steps(15) infinite;
  }
  @keyframes swell {
    from { opacity: 0; transform: scale(0.7) }
    to { opacity: calc(0.5 * var(--glow-power)); transform: scale(var(--burst)) }
  }
  @keyframes glowframes { to { -webkit-mask-position: -5100px 0; mask-position: -5100px 0 } }

  /* three bursts around the name rather than one on top of it */
  .sparks {
    width: 96px;
    height: 96px;
    -webkit-mask-image: var(--sparks);
    mask-image: var(--sparks);
    -webkit-mask-size: 1344px 96px;
    mask-size: 1344px 96px;
  }
  .sparks.left { margin-right: 210px; margin-top: -14px }
  .sparks.right { margin-left: 210px; margin-top: 18px }
  .sparks.over { margin-bottom: 74px; width: 72px; height: 72px }
  .fx.playing .sparks,
  .twfx.playing .sparks {
    animation: pop var(--in) ease-out forwards,
               vanish var(--out) ease-in var(--hold) forwards,
               /* the burst keeps going for as long as the thing is up */
               sparkframes 700ms steps(14) infinite;
  }
  .fx.playing .sparks.right,
  .twfx.playing .sparks.right { animation-delay: 140ms, var(--hold), 140ms }
  .fx.playing .sparks.over,
  .twfx.playing .sparks.over { animation-delay: 280ms, var(--hold), 280ms }
  @keyframes pop {
    from { opacity: 0; transform: scale(0.6) }
    to { opacity: 1; transform: scale(1) }
  }
  @keyframes sparkframes { to { -webkit-mask-position: -1344px 0; mask-position: -1344px 0 } }

  .caption {
    position: relative;
    display: flex;
    align-items: baseline;
    justify-content: center;
    flex-wrap: wrap;
    gap: 8px;
    font-size: calc(19px * var(--font-scale));
    white-space: nowrap;
    text-shadow: 0 2px 0 #000, 0 0 12px #000, 0 0 24px var(--tint);
    opacity: 0;
  }
  .fx.playing .caption {
    animation: rise var(--in) ease-out forwards,
               vanish var(--out) ease-in var(--hold) forwards;
  }
  @keyframes rise {
    from { opacity: 0; transform: translateY(8px) scale(0.94) }
    to { opacity: 1; transform: translateY(0) scale(1) }
  }
  .rar {
    color: var(--tint);
    font-size: 12px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .name { color: #f4e6bb }
  .grade {
    color: var(--tint);
    font-size: 12px;
    border: 1px solid var(--tint);
    padding: 0 4px;
  }
  .entrance-slam .fx.playing .caption {
    animation: slam var(--in) cubic-bezier(.17,.84,.22,1.25) forwards,
               vanish var(--out) ease-in var(--hold) forwards;
  }
  @keyframes slam {
    from { opacity: 0; transform: translateY(-34px) scale(1.3) }
    to { opacity: 1; transform: translateY(0) scale(1) }
  }
  .entrance-rift .fx.playing .caption {
    animation: caption-rift var(--in) cubic-bezier(.16,1,.3,1) forwards,
               vanish var(--out) ease-in var(--hold) forwards;
  }
  @keyframes caption-rift {
    from { opacity: 0; clip-path: inset(48% 0 48% 0); transform: scaleX(.6) }
    to { opacity: 1; clip-path: inset(0); transform: scaleX(1) }
  }
  .entrance-fade .fx.playing .caption,
  .reduced .fx.playing .caption {
    animation: appear var(--in) ease-out forwards,
               vanish var(--out) ease-in var(--hold) forwards;
  }
  .reduced .beam,
  .reduced .shockwave,
  .reduced .particle-field,
  .reduced .projectile-streaks,
  .reduced .vitality-pulse,
  .reduced .socket-orbit { display: none; }
  .statline {
    flex-basis: 100%;
    color: #d9faff;
    font-size: 12px;
    letter-spacing: 0.04em;
    text-align: center;
  }

  /* Twitch gets its own silhouette, not a recolored loot pillar. The clipped
     broadcast card and live badge are recognizable at a glance, while all
     viewer-supplied text remains ordinary Svelte text interpolation. */
  .twfx {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    transform: scale(var(--scale));
  }
  .twshade {
    position: absolute;
    inset: 0;
    opacity: 0;
    background: radial-gradient(ellipse 44% 48% at 50% 50%, rgba(0,0,0,var(--shade)), transparent 72%);
  }
  .twframe {
    position: relative;
    box-sizing: border-box;
    min-width: 410px;
    max-width: 540px;
    min-height: 102px;
    padding: 17px 54px 17px 82px;
    display: flex;
    align-items: center;
    color: #fff;
    opacity: 0;
    background:
      linear-gradient(135deg, color-mix(in srgb, var(--tint) 52%, #0b0710), rgba(11,7,16,.94) 46%, rgba(11,7,16,.87)),
      #0b0710;
    border: 1px solid color-mix(in srgb, var(--tint) 72%, white);
    box-shadow: 0 0 0 3px rgba(0,0,0,.55), 0 0 30px color-mix(in srgb, var(--tint) 38%, transparent);
    clip-path: polygon(16px 0, 100% 0, calc(100% - 16px) 100%, 0 100%, 0 16px);
  }
  .twframe::before,
  .twframe::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    height: 2px;
    background: linear-gradient(90deg, transparent, var(--tint), #fff, var(--tint), transparent);
    box-shadow: 0 0 10px var(--tint);
  }
  .twframe::before { top: 0; }
  .twframe::after { bottom: 0; }
  .twmark {
    position: absolute;
    left: 18px;
    width: 46px;
    height: 46px;
    display: grid;
    place-items: center;
    color: #fff;
    background: var(--tint);
    border-radius: 8px 8px 8px 2px;
    box-shadow: 0 0 18px var(--tint);
    transform: rotate(-4deg);
  }
  .twmark::after {
    content: '';
    position: absolute;
    right: 7px;
    bottom: -6px;
    width: 10px;
    height: 10px;
    background: var(--tint);
    clip-path: polygon(0 0, 100% 0, 0 100%);
  }
  .twmark span { font-size: 9px; letter-spacing: .13em; margin-right: -.13em; text-shadow: 0 1px #000; }
  .twcopy { min-width: 0; display: flex; align-items: baseline; flex-wrap: wrap; gap: 3px 9px; }
  .twheading {
    flex-basis: 100%;
    color: color-mix(in srgb, var(--tint) 42%, white);
    font-size: calc(11px * var(--font-scale));
    letter-spacing: .18em;
    text-transform: uppercase;
  }
  .twactor {
    min-width: 0;
    max-width: 330px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: #fff;
    font-size: calc(23px * var(--font-scale));
    text-shadow: 0 0 14px var(--tint);
  }
  .twdetail {
    color: color-mix(in srgb, var(--tint) 28%, white);
    font-size: calc(13px * var(--font-scale));
  }
  .twmessage {
    flex-basis: 100%;
    max-width: 390px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: #d8d1df;
    font-size: calc(11px * var(--font-scale));
  }
  .twpulse {
    position: absolute;
    right: 20px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #f33558;
    box-shadow: 0 0 0 0 rgba(243,53,88,.65);
  }
  .twfx.playing .twshade {
    animation: appear var(--in) ease-out forwards, vanish var(--out) ease-in var(--hold) forwards;
  }
  .twfx.playing .twframe {
    animation: tw-rise var(--in) cubic-bezier(.16,1,.3,1) forwards, vanish var(--out) ease-in var(--hold) forwards;
  }
  .entrance-slam .twfx.playing .twframe {
    animation: tw-slam var(--in) cubic-bezier(.17,.84,.22,1.25) forwards, vanish var(--out) ease-in var(--hold) forwards;
  }
  .entrance-rift .twfx.playing .twframe {
    animation: tw-in var(--in) cubic-bezier(.16,1,.3,1) forwards, vanish var(--out) ease-in var(--hold) forwards;
  }
  .entrance-fade .twfx.playing .twframe {
    animation: appear var(--in) ease-out forwards, vanish var(--out) ease-in var(--hold) forwards;
  }
  .twfx.playing .twpulse { animation: live-pulse 1.2s ease-out infinite; }
  @keyframes tw-rise {
    from { opacity: 0; transform: translateY(16px) scale(.94) }
    to { opacity: 1; transform: translateY(0) scale(1) }
  }
  @keyframes tw-slam {
    from { opacity: 0; transform: translateY(-34px) scale(1.18) }
    to { opacity: 1; transform: translateY(0) scale(1) }
  }
  @keyframes tw-in {
    from { opacity: 0; transform: translateX(36px) scale(.92); clip-path: polygon(50% 48%,50% 48%,50% 52%,50% 52%,50% 48%) }
    to { opacity: 1; transform: translateX(0) scale(1); clip-path: polygon(16px 0,100% 0,calc(100% - 16px) 100%,0 100%,0 16px) }
  }
  @keyframes live-pulse {
    0% { box-shadow: 0 0 0 0 rgba(243,53,88,.7) }
    100% { box-shadow: 0 0 0 12px rgba(243,53,88,0) }
  }
  .reduced .twfx.playing .twframe { animation: appear var(--in) ease-out forwards, vanish var(--out) ease-in var(--hold) forwards; }
  .reduced .twfx.playing .twpulse { animation: none; box-shadow: none; }

  /* only while it is being parked */
  /* It has to be unmistakable. Transparent and outlined in a thin dash, it was
     a window nobody could see grabbing clicks nobody could explain. */
  /* No fill: it is drawn after .fx with no z-index, so a full-bleed scrim
     painted over the very sample the player is here to judge. The dashed
     border and the button are enough to say what this box is. */
  .place {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    border: 2px dashed rgba(232, 216, 168, 0.75);
    box-sizing: border-box;
    cursor: move;
  }
  .hint {
    font-size: 13px;
    color: #e8d8a8;
    text-shadow: 0 1px 0 #000;
  }
  .placing-kind {
    padding: 4px 10px;
    color: #fff1b5;
    font-size: 15px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    background: rgba(0, 0, 0, 0.82);
    border: 1px solid #e8c860;
    box-shadow: 0 0 12px rgba(232, 200, 96, 0.28);
  }
  .place-actions { display: flex; flex-wrap: wrap; justify-content: center; gap: 7px; }
  .done {
    font: inherit;
    font-size: 13px;
    color: #e8d8a8;
    background: rgba(0, 0, 0, 0.85);
    border: 1px solid #8a7a5a;
    padding: 6px 20px;
    cursor: pointer;
  }
  .done:hover { border-color: #e8c860; }
  .done.all { color: #d8f8ff; border-color: #5b9baa; }
  .done.all:hover { border-color: #79ddef; }
  .done.cancel { color: #d3c5bd; border-color: #6a5550; }
  .done.cancel:hover { color: #fff; border-color: #a77a6f; }
  .place-error {
    max-width: min(460px, calc(100vw - 32px));
    padding: 5px 8px;
    color: #ffc0b2;
    font-size: 11px;
    line-height: 1.3;
    text-align: center;
    background: rgba(72, 8, 7, 0.88);
    border: 1px solid rgba(235, 91, 69, 0.72);
    text-shadow: 0 1px 0 #000;
  }
  .escape {
    color: #9a8a68;
    font-size: 10px;
    text-shadow: 0 1px 0 #000;
  }

  /* ── The satanic zone. ───────────────────────────────────────────────────
     A drop is a column of light; a rotation is a band that opens across the
     window. The axis, the edges and the sprites all differ on purpose: the two
     never share the screen, so the only thing telling them apart is the shape
     each leaves in the corner of the eye.

     Authored at 560x220 in fixed pixels. Not one percentage width in here —
     .zfx is inset:0 and then scaled, so `width: 100%` would be 1120px before
     the scale and 2240 after it. */
  .zfx {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    transform: scale(var(--scale));

    --ember: #ff3a2e;
    --ember-lit: #ffb08a;
    /* the token the panel's zone chip turns its name to when the zone moves.
       It carries a meaning rather than a season, so both skins agree on it. */
    --zone-ink: var(--rar-satanic, #ff6a6a);
  }
  .zfx.colossal {
    --ember: #42dcff;
    --ember-lit: #ffe98c;
    --zone-ink: #ffe477;
  }

  /* Height comes from what is in it — two buffs is a thinner band than five —
     and is capped, so a zone carrying more than the game has ever sent still
     cannot push past the window. */
  .rift {
    position: relative;
    box-sizing: border-box;
    width: 560px;
    max-height: 220px;
    padding: 10px 24px 12px;
    display: flex;
    flex-direction: column;
    align-items: center;
    overflow: hidden;
  }

  /* The slab. Its black is the player's own shade setting, so that slider still
     means something here. Masked at both ends: the window is 560 wide, and a
     bar that stops dead at the edge reads as a bug rather than as a band
     crossing the screen. */
  .band {
    position: absolute;
    inset: 0;
    transform-origin: 50% 50%;
    opacity: 0;
    overflow: hidden;
    background:
      radial-gradient(
        ellipse 70% 130% at 50% 50%,
        rgba(255, 58, 46, calc(var(--shade) * 0.2)) 0%,
        rgba(255, 58, 46, 0) 70%
      ),
      linear-gradient(
        180deg,
        rgba(0, 0, 0, 0) 0%,
        rgba(0, 0, 0, calc(var(--shade) * 1.35)) 14%,
        rgba(0, 0, 0, calc(var(--shade) * 1.55)) 50%,
        rgba(0, 0, 0, calc(var(--shade) * 1.35)) 86%,
        rgba(0, 0, 0, 0) 100%
      );
    -webkit-mask-image: linear-gradient(90deg, transparent 0, #000 10%, #000 90%, transparent 100%);
    mask-image: linear-gradient(90deg, transparent 0, #000 10%, #000 90%, transparent 100%);
  }
  .zfx.colossal .band {
    background:
      radial-gradient(
        ellipse 70% 130% at 50% 50%,
        rgba(56, 220, 255, calc(var(--shade) * 0.28)) 0%,
        rgba(255, 218, 91, calc(var(--shade) * 0.08)) 45%,
        rgba(56, 220, 255, 0) 72%
      ),
      linear-gradient(
        180deg,
        rgba(0, 0, 0, 0) 0%,
        rgba(0, 0, 0, calc(var(--shade) * 1.35)) 14%,
        rgba(0, 0, 0, calc(var(--shade) * 1.55)) 50%,
        rgba(0, 0, 0, calc(var(--shade) * 1.35)) 86%,
        rgba(0, 0, 0, 0) 100%
      );
  }
  /* The two lips, held 3px inside the slab so the bloom pools inward instead of
     being cut off by the clip. */
  .band::before,
  .band::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    height: 2px;
    background: var(--ember);
    box-shadow: 0 0 10px 1px var(--ember), 0 0 26px rgba(255, 58, 46, 0.55);
    animation: lip-breathe 2.4s ease-in-out infinite alternate;
  }
  .zfx.colossal .band::before,
  .zfx.colossal .band::after {
    box-shadow: 0 0 10px 1px var(--ember), 0 0 28px rgba(255, 221, 92, 0.62);
  }
  .band::before { top: 3px }
  .band::after { bottom: 3px }

  .zfx.playing .band {
    animation: rift-open var(--in) cubic-bezier(0.16, 1, 0.3, 1) forwards,
               rift-shut var(--out) ease-in var(--hold) forwards;
  }
  /* It arrives as the hairline the two lips make when they are together, and it
     leaves the same way. Nothing else in the app opens like this. */
  @keyframes rift-open {
    from { opacity: 0; transform: scaleY(0.06) }
    60% { opacity: 1 }
    to { opacity: 1; transform: scaleY(1) }
  }
  @keyframes rift-shut {
    from { opacity: 1; transform: scaleY(1) }
    to { opacity: 0; transform: scaleY(0.06) }
  }
  @keyframes lip-breathe {
    from { opacity: 0.75 }
    to { opacity: 1 }
  }

  /* The sweep the panel's zone chip already gets when the zone moves, borrowed
     so the two say the same thing. Slower and a third of the brightness: there
     it crosses 240px for three seconds and is meant to nag, here it crosses 560
     for six and is meant to be lived with. */
  .sweep {
    position: absolute;
    inset: 0;
    pointer-events: none;
    background: linear-gradient(100deg, transparent 30%, rgba(255, 255, 255, 0.14) 50%, transparent 70%);
    animation: rift-sweep 2.6s linear infinite;
  }
  @keyframes rift-sweep {
    from { transform: translateX(-100%) }
    to { transform: translateX(100%) }
  }

  /* A sibling of the slab, not a child: the slab scales on its way in, and a
     scaling parent would squash the text with it. One fade takes the column out
     together at the end. */
  .zbody {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
  }
  .zfx.playing .zbody {
    animation: vanish var(--out) ease-in var(--hold) forwards;
  }

  .zkind {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 23px;
    opacity: 0;
  }
  /* native 23x23 and drawn at 23: this is pixel art, and a fractional size
     turns the pentagram to mush */
  .zkind img {
    width: 23px;
    height: 23px;
    image-rendering: pixelated;
    filter: drop-shadow(0 0 6px var(--ember));
  }
  .zkind .txt {
    font-size: 11px;
    letter-spacing: 0.34em;
    /* letter-spacing hangs a gap off the last letter, which walks the whole
       line half a space left of centre */
    margin-right: -0.34em;
    text-transform: uppercase;
    color: var(--ember-lit);
    text-shadow: 0 2px 0 #000, 0 0 14px var(--ember);
  }
  .zkind .zcurse {
    font-size: 10px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: #9a6a6a;
    text-shadow: 0 1px 0 #000;
  }
  .zfx.colossal .zkind .zcurse { color: #d9c77d; }
  .zfx.playing .zkind {
    animation: fall-in var(--in) ease-out 60ms forwards;
  }
  /* down from above — the drop's caption rises from below, and that alone reads
     before either has been focused on */
  @keyframes fall-in {
    from { opacity: 0; transform: translateY(-10px) }
    to { opacity: 1; transform: translateY(0) }
  }

  /* The panel's own zone plate at twice the width. The player already knows
     this shape means "zone"; nothing else in this window wears it. */
  .zplate {
    box-sizing: border-box;
    width: 384px;
    height: 40px;
    margin-top: 6px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 18px;
    background-image: var(--plate);
    background-size: 100% 100%;
    background-repeat: no-repeat;
    image-rendering: pixelated;
    transform-origin: 50% 50%;
    opacity: 0;
  }
  .zname {
    font-size: 19px;
    line-height: 1;
    color: var(--zone-ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-shadow: 0 2px 0 #000, 0 0 16px rgba(255, 58, 46, 0.8);
  }
  .zfx.colossal .zname {
    text-shadow: 0 2px 0 #000, 0 0 10px rgba(66, 220, 255, 0.95), 0 0 20px rgba(255, 224, 104, 0.55);
  }
  .zfx.playing .zplate {
    animation: plate-in var(--in) cubic-bezier(0.2, 0.9, 0.2, 1) 120ms forwards;
  }
  /* widens rather than swells: the drop's glow grows from a point in every
     direction, this one runs out along the band */
  @keyframes plate-in {
    from { opacity: 0; transform: scaleX(0.66) }
    to { opacity: 1; transform: scaleX(1) }
  }

  /* Two fixed columns, so five buffs are three rows and the longest name in the
     table still fits without an ellipsis. Fixed rather than content-sized: a
     ragged pair of edges is a caption, a squared-off pair is a sheet, and a
     sheet is what is being read. */
  .zbuffs {
    display: grid;
    grid-template-columns: repeat(2, 210px);
    gap: 6px 14px;
    justify-content: center;
    margin-top: 10px;
  }
  .zbuff {
    box-sizing: border-box;
    min-width: 0;
    height: 33px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    border: 6px solid transparent;
    border-image-source: var(--chipart);
    border-image-slice: 6 fill;
    border-image-width: 6px;
    image-rendering: pixelated;
    white-space: nowrap;
    opacity: 0;
  }
  /* an odd one out sits under the middle rather than hanging off the left */
  .zbuff:last-child:nth-child(odd) {
    grid-column: 1 / -1;
    justify-self: center;
    width: max-content;
  }
  .zbuff img {
    width: 21px;
    height: 21px;
    flex: none;
    image-rendering: pixelated;
  }
  .zbuff .bname {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
    color: #f4e6bb;
    text-shadow: 0 1px 0 #000;
  }
  .zbuff.more .bname { color: var(--ember-lit) }
  .znone {
    margin-top: 10px;
    height: 33px;
    display: flex;
    align-items: center;
    font-size: 12px;
    color: #9a6a6a;
    text-shadow: 0 1px 0 #000;
    opacity: 0;
  }
  .zfx.playing .zbuff,
  .zfx.playing .znone {
    animation: chip-in 260ms ease-out 200ms forwards;
  }
  /* one after another, so the list is read as a list. The last starts at 400ms
     and has settled by 660 — the shortest hold this window allows is 1400, so
     it never collides with the fade out. */
  .zfx.playing .zbuff:nth-child(2) { animation-delay: 240ms }
  .zfx.playing .zbuff:nth-child(3) { animation-delay: 280ms }
  .zfx.playing .zbuff:nth-child(4) { animation-delay: 320ms }
  .zfx.playing .zbuff:nth-child(5) { animation-delay: 360ms }
  .zfx.playing .zbuff:nth-child(6) { animation-delay: 400ms }
  @keyframes chip-in {
    from { opacity: 0; transform: translateY(6px) }
    to { opacity: 1; transform: translateY(0) }
  }

  /* See main.js, and the shade above. Twenty paints of a half-transparent black
     on a desktop that never clears its surface arrive as a hard blob, so the
     band is simply there and simply gone. The text keeps its fades: it is
     small, and the caption already does the same. */
  :global(html[data-os='linux']) .zfx.playing .band {
    animation: none;
    opacity: 1;
    transform: none;
  }

  @media (prefers-reduced-motion: reduce) {
    .zfx.playing .band {
      animation: appear var(--in) ease-out forwards,
                 vanish var(--out) ease-in var(--hold) forwards;
    }
    .band::before,
    .band::after { animation: none; opacity: 1 }
    .sweep { animation: none; opacity: 0.1 }
    .zfx.playing .zkind,
    .zfx.playing .zplate,
    .zfx.playing .zbuff,
    .zfx.playing .znone {
      animation: appear var(--in) ease-out forwards;
    }
  }
</style>
