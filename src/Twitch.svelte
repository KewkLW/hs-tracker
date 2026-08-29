<script>
  import { invoke, listen } from './bridge.js';
  import { art } from './skin.svelte.js';
  import { STARTER_FX_PRESETS } from './fx-presets.js';
  import {
    TWITCH_ALERT_BY_KIND,
    TWITCH_ALERT_CATALOG,
    TWITCH_ALERT_GROUPS,
    TWITCH_SOUND_OPTIONS,
    cleanTwitchClientId,
    logicalTwitchAlertKind,
    normaliseTwitchAlerts,
    requiredTwitchScopes,
    sameTwitchSettingsSnapshot,
  } from './twitch-config.js';

  let settings = $state(null);
  let alerts = $state(normaliseTwitchAlerts());
  let status = $state({ state: 'disconnected' });
  let auth = $state(null);
  let authLeft = $state(0);
  let authBusy = $state(false);
  let testing = $state('');
  let query = $state('');
  let notice = $state('');
  let lastAlert = $state(null);

  let saveTimer = null;
  let pendingSettings = null;
  let saveDrain = null;
  let saveInFlight = null;
  let inFlightSnapshot = null;
  let authPollTimer = null;
  let countdownTimer = null;
  let healthTimer = null;
  let noticeTimer = null;

  let neededScopes = $derived(requiredTwitchScopes(alerts));
  let grantedScopes = $derived.by(() => {
    const value = status?.granted_scopes ?? status?.scopes ?? status?.auth?.scopes ?? [];
    return Array.isArray(value) ? value.map(String).sort() : [];
  });
  let missingScopes = $derived(neededScopes.filter((scope) => !grantedScopes.includes(scope)));
  let savedFxPresets = $derived(Array.isArray(settings?.flourish_fx_presets) ? settings.flourish_fx_presets : []);
  let connected = $derived(Boolean(status?.connected ?? status?.authenticated ?? ['connected', 'ready', 'live'].includes(status?.state)));
  let authorized = $derived(Boolean(
    status?.authorized
      || status?.authenticated
      || connected
      || status?.display_name
      || status?.account?.display_name
      || status?.login
      || status?.account?.login
      || grantedScopes.length
      || ['connecting', 'reconnecting', 'error'].includes(String(status?.state ?? '').toLowerCase()),
  ));
  let authPending = $derived(Boolean(auth && !['expired', 'denied'].includes(auth.state)));
  let connectionState = $derived(String(status?.state ?? (connected ? 'connected' : 'disconnected')).replaceAll('_', ' '));
  let accountName = $derived(status?.display_name ?? status?.account?.display_name ?? status?.login ?? status?.account?.login ?? 'Not connected');

  function flash(message) {
    clearTimeout(noticeTimer);
    notice = String(message ?? '');
    noticeTimer = setTimeout(() => (notice = ''), 5000);
  }

  function hydrate(value) {
    if (!value) return;
    settings = value;
    settings.twitch_client_id = cleanTwitchClientId(value.twitch_client_id);
    settings.twitch_enabled = Boolean(value.twitch_enabled);
    alerts = normaliseTwitchAlerts(value.twitch_alerts);
  }

  function localTwitchFields() {
    return {
      twitch_enabled: Boolean(settings?.twitch_enabled),
      twitch_client_id: cleanTwitchClientId(settings?.twitch_client_id),
      twitch_alerts: $state.snapshot(alerts),
    };
  }

  function settingsSnapshot(base = settings) {
    return $state.snapshot({ ...base, ...localTwitchFields() });
  }

  function sameSnapshot(left, right) {
    // serde_json stores map keys in sorted order, whereas the UI constructs
    // twitch_alerts in catalog order. Compare values canonically or every
    // successful save looks like a foreign edit and queues itself forever.
    return sameTwitchSettingsSnapshot(left, right);
  }

  async function refreshStatus() {
    const next = await invoke('twitch_status').catch((error) => ({ state: 'error', error: String(error) }));
    if (next) status = next;
  }

  $effect(() => {
    invoke('get_settings').then(hydrate).catch((error) => flash(error));
    refreshStatus();

    const unsubs = [
      listen('settings-changed', (event) => {
        if ((saveTimer || pendingSettings || saveDrain || saveInFlight) && settings) {
          const ownSaveEcho = sameSnapshot(event.payload, inFlightSnapshot);
          // Preserve a newer queued merge when the backend echoes an older
          // snapshot that this tab just sent. For all other events, take the
          // fresh unrelated fields and lay the unsaved Twitch controls over it.
          if (!ownSaveEcho || (!pendingSettings && !saveTimer)) {
            settings = {
              ...event.payload,
              ...localTwitchFields(),
            };
          }
          // If another tab saved while our complete Settings snapshot was in
          // flight, queue one merged follow-up. A settings event identical to
          // the snapshot we just wrote is our own echo and must not create an
          // endless save loop.
          if (!ownSaveEcho) pendingSettings = settingsSnapshot();
        } else {
          hydrate(event.payload);
        }
      }),
      listen('twitch-status', (event) => {
        if (event?.payload) status = event.payload;
      }),
      listen('twitch-alert', (event) => {
        lastAlert = event?.payload ?? null;
      }),
    ];

    healthTimer = setInterval(refreshStatus, 20000);
    const beforeUnload = () => { void flushSave(); };
    window.addEventListener('beforeunload', beforeUnload);
    return () => {
      // Switching tabs can happen inside the 150 ms debounce. Start the write
      // before this component disappears so the final control edit is kept.
      void flushSave();
      clearTimeout(authPollTimer);
      clearInterval(countdownTimer);
      clearInterval(healthTimer);
      clearTimeout(noticeTimer);
      window.removeEventListener('beforeunload', beforeUnload);
      unsubs.forEach((pending) => pending.then((stop) => stop()));
    };
  });

  function stageSettings() {
    if (!settings) return;
    settings.twitch_client_id = cleanTwitchClientId(settings.twitch_client_id);
    settings.twitch_enabled = Boolean(settings.twitch_enabled);
    alerts = normaliseTwitchAlerts($state.snapshot(alerts));
    settings.twitch_alerts = $state.snapshot(alerts);
    pendingSettings = settingsSnapshot();
  }

  function persist() {
    if (!settings) return;
    stageSettings();
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      saveTimer = null;
      void flushSave();
    }, 150);
  }

  async function flushSave(stageCurrent = false) {
    if (stageCurrent) stageSettings();
    clearTimeout(saveTimer);
    saveTimer = null;

    // Every caller, including Test, waits for the same drain. A second edit
    // made while a write is running is picked up by this loop rather than sent
    // concurrently as another stale complete-settings snapshot.
    if (!saveDrain) {
      saveDrain = (async () => {
        let succeeded = true;
        while (pendingSettings) {
          const snapshot = pendingSettings;
          pendingSettings = null;
          inFlightSnapshot = snapshot;
          const running = invoke('save_settings', { settings: snapshot });
          saveInFlight = running;
          try {
            await running;
          } catch (error) {
            succeeded = false;
            flash(error);
          } finally {
            if (saveInFlight === running) {
              saveInFlight = null;
              inFlightSnapshot = null;
            }
          }
        }
        return succeeded;
      })();
    }
    const activeDrain = saveDrain;
    let succeeded;
    try {
      succeeded = await activeDrain;
    } finally {
      if (saveDrain === activeDrain) saveDrain = null;
    }
    // A caller can stage data in the narrow interval after the drain's last
    // loop check. Start another drain before reporting that everything saved.
    return pendingSettings ? (await flushSave()) && succeeded : succeeded;
  }

  function toggleTracker() {
    settings.twitch_enabled = !settings.twitch_enabled;
    persist();
  }

  function toggleAlert(kind) {
    alerts[kind].enabled = !alerts[kind].enabled;
    persist();
  }

  function setGroup(group, enabled) {
    for (const alert of TWITCH_ALERT_CATALOG) {
      if (alert.group === group) alerts[alert.kind].enabled = enabled;
    }
    persist();
  }

  function cleanClientId() {
    settings.twitch_client_id = cleanTwitchClientId(settings.twitch_client_id);
    persist();
  }

  function fxPresetExists(value) {
    return value === 'current'
      || STARTER_FX_PRESETS.some((preset) => preset.id === value)
      || savedFxPresets.some((preset) => preset.id === value);
  }

  function matchingGroup(group) {
    const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    return TWITCH_ALERT_CATALOG.filter((alert) => {
      if (alert.group !== group) return false;
      if (!terms.length) return true;
      const haystack = `${alert.label} ${alert.kind} ${alert.description} ${alert.placeholders.join(' ')}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }

  function resetAlert(kind) {
    const clean = normaliseTwitchAlerts({})[kind];
    alerts[kind] = clean;
    persist();
    flash(`Reset ${TWITCH_ALERT_BY_KIND.get(kind)?.label ?? kind}`);
  }

  function stopAuth() {
    clearTimeout(authPollTimer);
    clearInterval(countdownTimer);
    authPollTimer = null;
    countdownTimer = null;
    authBusy = false;
  }

  function startCountdown(seconds) {
    clearInterval(countdownTimer);
    authLeft = Math.max(0, Number(seconds) || 0);
    countdownTimer = setInterval(() => {
      authLeft = Math.max(0, authLeft - 1);
      if (!authLeft) {
        stopAuth();
        if (auth) auth = { ...auth, state: 'expired' };
      }
    }, 1000);
  }

  function queueAuthPoll(delaySeconds) {
    clearTimeout(authPollTimer);
    authPollTimer = setTimeout(pollAuth, Math.max(1, Number(delaySeconds) || 5) * 1000);
  }

  async function beginAuth() {
    const clientId = cleanTwitchClientId(settings?.twitch_client_id);
    if (!clientId) {
      flash('Paste the Client ID from a Twitch Public application first.');
      return;
    }
    cleanClientId();
    if (!settings.twitch_enabled) {
      settings.twitch_enabled = true;
      persist();
    }
    // Authorization can complete quickly on an already signed-in browser.
    // Commit the client ID and enabled state first so the Rust poll handler
    // always sees the same configuration when it starts EventSub.
    if (!(await flushSave(true))) return;
    stopAuth();
    authBusy = true;
    try {
      const next = await invoke('twitch_begin_auth', { clientId, scopes: neededScopes });
      // The polling device_code deliberately remains inside Rust. It has no
      // reason to cross IPC; this window only needs the public one-time code.
      if (!next?.user_code || !next?.verification_uri) throw new Error('Twitch did not return an activation code.');
      auth = { ...next, state: 'pending' };
      startCountdown(next.expires_in);
      queueAuthPoll(next.interval);
    } catch (error) {
      authBusy = false;
      flash(error);
    }
  }

  async function pollAuth() {
    if (!auth || authLeft <= 0) return;
    authBusy = true;
    try {
      const result = await invoke('twitch_poll_auth');
      const state = result?.state ?? 'pending';
      auth = { ...auth, state };
      if (state === 'connected') {
        stopAuth();
        if (result.status) status = result.status;
        else await refreshStatus();
        auth = null;
        flash(`Connected as ${status?.display_name ?? status?.login ?? 'your Twitch account'}`);
        return;
      }
      if (state === 'expired' || state === 'denied') {
        stopAuth();
        flash(state === 'denied' ? 'Twitch authorization was declined.' : 'The Twitch code expired.');
        return;
      }
      const base = Number(auth.interval) || 5;
      queueAuthPoll(state === 'slow_down' ? base + 5 : base);
    } catch (error) {
      // A momentary network failure should not throw away a still-valid code.
      flash(error);
      queueAuthPoll(Number(auth.interval) || 5);
    } finally {
      authBusy = false;
    }
  }

  async function copyCode() {
    if (!auth?.user_code) return;
    try {
      await navigator.clipboard.writeText(auth.user_code);
      flash('Activation code copied.');
    } catch {
      flash('Could not copy automatically. Select the code and copy it.');
    }
  }

  async function disconnect() {
    stopAuth();
    auth = null;
    await invoke('twitch_disconnect').catch((error) => flash(error));
    await refreshStatus();
  }

  async function restart() {
    await invoke('twitch_restart').catch((error) => flash(error));
    await refreshStatus();
  }

  async function testAlert(kind) {
    testing = kind;
    try {
      // A click moves focus out of a text/number input and can beat the
      // debounced save by a frame. Test only after the exact visible settings
      // have reached Rust, so text, sound, volume and FX all match the preview.
      if (!(await flushSave(true))) return;
      await invoke('twitch_test_alert', { kind });
    } catch (error) {
      flash(error);
    } finally {
      setTimeout(() => { if (testing === kind) testing = ''; }, 400);
    }
  }

  function formatTime(value) {
    if (!value) return 'Never';
    const numeric = typeof value === 'number' || /^\d+(?:\.\d+)?$/.test(String(value)) ? Number(value) : NaN;
    // Rust commonly sends Unix seconds while Date expects milliseconds. Large
    // values are already millisecond timestamps and ISO strings pass through.
    const date = new Date(Number.isFinite(numeric) ? (numeric < 1e12 ? numeric * 1000 : numeric) : value);
    return Number.isNaN(date.valueOf()) ? String(value) : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function twitchVerificationUri(value) {
    try {
      const url = new URL(String(value ?? ''));
      const host = url.hostname.toLowerCase();
      const path = url.pathname.replace(/\/+$/, '');
      if (
        url.protocol !== 'https:'
        || (host !== 'twitch.tv' && host !== 'www.twitch.tv')
        || path !== '/activate'
        || url.username
        || url.password
        || url.port
        || url.search
        || url.hash
      ) return '';
      return url.href;
    } catch {
      return '';
    }
  }

  async function openActivation() {
    const url = twitchVerificationUri(auth?.verification_uri);
    if (!url) {
      flash('Twitch returned an invalid activation address.');
      return;
    }
    await invoke('open_url', { url }).catch((error) => flash(error));
  }

  function recentAlertLabel(value) {
    if (!value) return 'No Twitch alert received this session';
    const rawKind = value.kind ?? value.alert_kind ?? 'event';
    const kind = logicalTwitchAlertKind(rawKind) || rawKind;
    const label = TWITCH_ALERT_BY_KIND.get(kind)?.label ?? String(rawKind).replaceAll('_', ' ');
    const user = value.user ?? value.user_name ?? value.display_name;
    return `${label}${user ? ` · ${user}` : ''}`;
  }
</script>

<div class="twitch-page">
  {#if settings}
    <section class="mast" style:border-image-source="url({art('chip_dark')})">
      <div class="brand">
        <div class="twitch-mark">T</div>
        <div>
          <div class="title">Twitch Alerts</div>
          <div class="hint">Native EventSub alerts rendered through the HS Tracker FX system.</div>
        </div>
      </div>
      <button class="master" onclick={toggleTracker} aria-pressed={settings.twitch_enabled}>
        <img src={settings.twitch_enabled ? art('check_on') : art('check_off')} alt="" />
        <span>{settings.twitch_enabled ? 'Alert engine enabled' : 'Alert engine disabled'}</span>
      </button>
    </section>

    <div class="top-grid">
      <section class="section connect" style:border-image-source="url({art('chip_dark')})">
        <div class="sechead">Connect safely</div>
        <div class="security">Public desktop app only. Paste a <b>Client ID</b> — never a client secret or OAuth token.</div>
        <label class="client-line">
          <span>Public Client ID</span>
          <input
            class="field client-id"
            type="text"
            autocomplete="off"
            spellcheck="false"
            maxlength="64"
            bind:value={settings.twitch_client_id}
            onchange={cleanClientId}
            placeholder="Twitch developer Client ID"
          />
        </label>
        <div class="connect-actions">
          <button class="btn purple" disabled={authBusy || authPending || !settings.twitch_client_id} onclick={beginAuth}>
            {authBusy && !auth ? 'Contacting Twitch…' : authPending ? 'Awaiting approval…' : authorized ? 'Authorize again' : 'Connect with device code'}
          </button>
          <button class="btn" disabled={!settings.twitch_enabled || !authorized} onclick={restart}>Restart listener</button>
          <button class="btn danger" disabled={!authorized && !auth} onclick={disconnect}>Disconnect</button>
        </div>

        {#if auth}
          <div class="device-card" class:ended={auth.state === 'expired' || auth.state === 'denied'}>
            <div>
              <div class="device-label">Enter this one-time code on Twitch</div>
              <button class="device-code" onclick={copyCode} title="Copy code">{auth.user_code}</button>
              <div class="device-time">{auth.state === 'slow_down' ? 'Waiting for Twitch… ' : ''}{authLeft}s remaining</div>
            </div>
            <button class="btn purple activate" onclick={openActivation}>Open Twitch activation</button>
          </div>
        {/if}
      </section>

      <section class="section health" style:border-image-source="url({art('chip_dark')})">
        <div class="sechead">Connection & health</div>
        <div class="health-grid">
          <div class="health-row"><span>State</span><strong class:good={connected} class:bad={status?.state === 'error'}>{connectionState}</strong></div>
          <div class="health-row"><span>Account</span><strong>{accountName}</strong></div>
          <div class="health-row"><span>WebSocket</span><strong>{status?.websocket_state ?? status?.websocket ?? (connected ? 'starting' : 'off')}</strong></div>
          <div class="health-row"><span>Subscriptions</span><strong>{status?.subscription_count ?? status?.subscriptions ?? 0}</strong></div>
          <div class="health-row"><span>Last EventSub</span><strong>{formatTime(status?.last_event_at ?? status?.last_message_at)}</strong></div>
          <div class="health-row"><span>Last validation</span><strong>{formatTime(status?.last_validation_at ?? status?.validated_at)}</strong></div>
        </div>
        {#if status?.error}<div class="error">{status.error}</div>{/if}
        <div class="last-event">{recentAlertLabel(lastAlert)}</div>
      </section>
    </div>

    <section class="section scopes" style:border-image-source="url({art('chip_dark')})">
      <div class="scope-head">
        <div>
          <div class="sechead">Permissions</div>
          <div class="hint">Calculated from enabled alerts. Turning on a new permission may require authorizing again.</div>
        </div>
        <span class="scope-count" class:warn={connected && missingScopes.length}>{neededScopes.length} needed · {grantedScopes.length} granted</span>
      </div>
      {#if neededScopes.length}
        <div class="scope-list">
          {#each neededScopes as scope}
            <span class="scope" class:missing={connected && !grantedScopes.includes(scope)}>{scope}</span>
          {/each}
        </div>
      {:else}
        <div class="hint">The currently enabled alerts do not require privileged scopes.</div>
      {/if}
      {#if connected && missingScopes.length}
        <div class="warning">Reconnect to grant: {missingScopes.join(', ')}</div>
      {/if}
    </section>

    <section class="section catalog" style:border-image-source="url({art('chip_dark')})">
      <div class="catalog-head">
        <div>
          <div class="sechead">Alert catalog</div>
          <div class="hint">Every Twitch-native alert is independently styled, filtered and testable. Threshold 0 means no minimum.</div>
        </div>
        <input class="field search" type="search" bind:value={query} placeholder="Search alerts…" aria-label="Search Twitch alerts" />
      </div>

      {#each TWITCH_ALERT_GROUPS as group}
        {@const rows = matchingGroup(group.id)}
        {#if rows.length}
          <div class="group">
            <div class="group-head">
              <div><span>{group.label}</span><small>{group.description}</small></div>
              <div class="group-actions">
                <button onclick={() => setGroup(group.id, true)}>All on</button>
                <button onclick={() => setGroup(group.id, false)}>All off</button>
              </div>
            </div>

            {#each rows as alert (alert.kind)}
              {@const config = alerts[alert.kind]}
              <details class="alert" class:on={config.enabled}>
                <summary>
                  <button class="check" onclick={(event) => { event.preventDefault(); toggleAlert(alert.kind); }} aria-label={`Toggle ${alert.label}`}>
                    <img src={config.enabled ? art('check_on') : art('check_off')} alt="" />
                  </button>
                  <span class="alert-name">{alert.label}</span>
                  <span class="alert-desc">{alert.description}</span>
                  <span class="threshold-summary">≥ {config.threshold} {alert.thresholdUnit}</span>
                  <button class="btn mini" class:testing={testing === alert.kind} onclick={(event) => { event.preventDefault(); testAlert(alert.kind); }}>Test</button>
                  <span class="chevron">⌄</span>
                </summary>
                <div class="alert-body">
                  <label class="control threshold">
                    <span>Minimum</span>
                    <input type="number" min="0" max="1000000000" step="1" bind:value={config.threshold} onchange={persist} />
                    <small>{alert.thresholdUnit}{config.threshold === 0 ? ' · any' : ''}</small>
                  </label>
                  <label class="control message">
                    <span>Alert text</span>
                    <input class="field" maxlength="240" bind:value={config.text} onchange={persist} />
                    <small>Available: {alert.placeholders.map((name) => `{${name}}`).join(' ') || 'plain text'}</small>
                  </label>
                  <label class="control">
                    <span>FX preset</span>
                    <select class="picker" bind:value={config.fx_preset} onchange={persist}>
                      {#if !fxPresetExists(config.fx_preset)}<option value={config.fx_preset}>Missing preset</option>{/if}
                      <option value="current">Current FX Lab look</option>
                      <optgroup label="Starter presets">
                        {#each STARTER_FX_PRESETS as preset}<option value={preset.id}>{preset.name}</option>{/each}
                      </optgroup>
                      {#if savedFxPresets.length}
                        <optgroup label="My saved presets">
                          {#each savedFxPresets as preset}<option value={preset.id}>{preset.name}</option>{/each}
                        </optgroup>
                      {/if}
                    </select>
                  </label>
                  <label class="control sound">
                    <span>Sound</span>
                    <select class="picker" bind:value={config.sound} onchange={persist}>
                      {#each TWITCH_SOUND_OPTIONS as [value, label]}<option value={value}>{label}</option>{/each}
                    </select>
                    <input type="range" min="0" max="1" step="0.05" bind:value={config.volume} oninput={persist} disabled={config.sound === 'none'} aria-label={`${alert.label} volume`} />
                    <small>{Math.round(config.volume * 100)}%</small>
                  </label>
                  <div class="alert-notes">
                    <span>EventSub: {alert.eventsub.join(', ')}</span>
                    {#if alert.overlap}<span class="overlap">Deduplication: {alert.overlap}</span>{/if}
                  </div>
                  <div class="row-actions">
                    <button class="link" onclick={() => resetAlert(alert.kind)}>Reset this alert</button>
                    <button class="btn" class:testing={testing === alert.kind} onclick={() => testAlert(alert.kind)}>Test visual + sound</button>
                  </div>
                </div>
              </details>
            {/each}
          </div>
        {/if}
      {/each}
    </section>

    {#if notice}<div class="toast">{notice}</div>{/if}
  {:else}
    <div class="empty">Loading Twitch settings…</div>
  {/if}
</div>

<style>
  @font-face {
    font-family: 'CookieRun Bold';
    src: url('./assets/fonts/cookierunbold.ttf') format('truetype');
  }

  .twitch-page {
    box-sizing: border-box;
    width: 100%;
    height: 100%;
    overflow-y: auto;
    padding: 0 2px 12px 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    color: var(--bone-6);
    font-family: 'CookieRun Bold', sans-serif;
    font-size: 12px;
  }
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
    min-height: 60px;
    padding: 6px 9px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .brand { display: flex; align-items: center; gap: 9px; min-width: 0; }
  .twitch-mark {
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    color: white;
    background: #9146ff;
    border: 1px solid #bd93ff;
    box-shadow: 0 0 14px rgba(145, 70, 255, 0.5);
    clip-path: polygon(0 0, 100% 0, 100% 76%, 72% 76%, 58% 100%, 58% 76%, 0 76%);
  }
  .title { color: var(--bone-13); font-size: 15px; }
  .hint { color: var(--dim-2); font-size: 10px; line-height: 1.35; }
  .master,
  .check,
  .link,
  .group-actions button {
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
    cursor: pointer;
  }
  .master { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
  .master:hover { color: var(--bone-13); }
  .master img,
  .check img { width: 18px; height: 18px; image-rendering: pixelated; }

  .top-grid { display: grid; grid-template-columns: minmax(360px, 1.35fr) minmax(280px, 0.8fr); gap: 8px; }
  .section { padding: 6px 8px 8px; min-width: 0; }
  .sechead { color: var(--edge-2b); font-size: 10px; letter-spacing: 0.35px; text-transform: uppercase; }
  .security {
    margin: 4px 0 3px;
    padding: 5px 7px;
    color: #d9c7ff;
    font-size: 10px;
    line-height: 1.35;
    background: rgba(104, 52, 170, 0.18);
    border-left: 2px solid #9146ff;
  }
  .client-line { display: grid; grid-template-columns: 94px minmax(140px, 1fr); align-items: center; gap: 7px; margin-top: 5px; }
  .client-id { font-family: Consolas, monospace !important; letter-spacing: 0.4px; }
  .connect-actions { display: flex; flex-wrap: wrap; gap: 5px; margin-top: 6px; }

  .device-card {
    margin-top: 7px;
    padding: 8px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    background: rgba(79, 34, 139, 0.22);
    border: 1px solid #69409e;
  }
  .device-card.ended { opacity: 0.5; }
  .device-label,
  .device-time { color: var(--dim-2); font-size: 9px; }
  .device-code { padding: 2px 0; font: 19px Consolas, monospace; letter-spacing: 3px; color: white; background: none; border: 0; cursor: copy; }
  .activate { text-decoration: none; white-space: nowrap; }

  .health-grid { display: grid; gap: 1px; margin-top: 4px; }
  .health-row { display: flex; justify-content: space-between; gap: 10px; min-height: 19px; align-items: center; border-bottom: 1px solid rgba(80, 58, 58, 0.35); }
  .health-row span { color: var(--dim-2); font-size: 10px; }
  .health-row strong { min-width: 0; color: var(--bone-9); font-size: 10px; font-weight: normal; text-transform: capitalize; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .health-row strong.good { color: #52dc7a; }
  .health-row strong.bad,
  .error { color: #ff7777; }
  .error { margin-top: 5px; font-size: 10px; line-height: 1.35; }
  .last-event { margin-top: 5px; color: #b69bdc; font-size: 9px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .scope-head,
  .catalog-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  .scope-count { color: #52dc7a; font-size: 10px; white-space: nowrap; }
  .scope-count.warn { color: #ffbd66; }
  .scope-list { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 6px; }
  .scope { padding: 2px 5px; color: #cbb8e9; font: 9px Consolas, monospace; border: 1px solid #46315d; background: rgba(49, 25, 76, 0.3); }
  .scope.missing { color: #ffbd66; border-color: #9d6534; }
  .warning { margin-top: 5px; color: #ffbd66; font-size: 9px; }

  .catalog { gap: 7px; }
  .search { width: min(220px, 40vw); }
  .group { margin-top: 8px; border: 1px solid var(--ground-10); background: rgba(0, 0, 0, 0.12); }
  .group-head { min-height: 34px; padding: 4px 7px; display: flex; align-items: center; justify-content: space-between; gap: 10px; background: rgba(0, 0, 0, 0.22); border-bottom: 1px solid var(--ground-10); }
  .group-head > div:first-child { display: flex; align-items: baseline; gap: 7px; min-width: 0; }
  .group-head span { color: var(--bone-9); }
  .group-head small { color: var(--dim-2); font: inherit; font-size: 9px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .group-actions { display: flex; gap: 6px; flex: none; }
  .group-actions button { padding: 1px 2px; color: var(--edge-2b); font-size: 9px; text-decoration: underline; }
  .group-actions button:hover { color: var(--bone-13); }

  details.alert { border-bottom: 1px solid rgba(62, 42, 44, 0.75); }
  details.alert:last-child { border-bottom: 0; }
  details.alert.on { background: rgba(86, 34, 98, 0.1); }
  details.alert > summary {
    min-height: 34px;
    padding: 3px 6px;
    display: grid;
    grid-template-columns: 21px minmax(115px, 0.55fr) minmax(130px, 1.4fr) 94px 44px 13px;
    align-items: center;
    gap: 6px;
    list-style: none;
    cursor: pointer;
  }
  details.alert > summary::-webkit-details-marker { display: none; }
  details.alert > summary:hover { background: rgba(112, 67, 124, 0.11); }
  .check { display: grid; place-items: center; padding: 0; }
  .alert-name { min-width: 0; color: var(--bone-8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .alert.on .alert-name { color: var(--bone-13); }
  .alert-desc { min-width: 0; color: var(--dim-2); font-size: 9px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .threshold-summary { color: #a989c9; font-size: 9px; text-align: right; white-space: nowrap; }
  .chevron { color: var(--edge-5); font-size: 13px; transform-origin: center; }
  details[open] .chevron { transform: rotate(180deg); }

  .alert-body { padding: 7px 10px 8px 34px; display: grid; grid-template-columns: minmax(145px, 0.7fr) minmax(240px, 1.5fr); gap: 6px 12px; border-top: 1px solid rgba(62, 42, 44, 0.5); background: rgba(0, 0, 0, 0.15); }
  .control { min-width: 0; display: grid; grid-template-columns: 70px minmax(80px, 1fr); align-items: center; gap: 5px; }
  .control > span { color: var(--bone-4); font-size: 10px; }
  .control small { grid-column: 2; color: var(--dim-2); font: inherit; font-size: 8px; line-height: 1.25; overflow-wrap: anywhere; }
  .control input[type='number'] { width: 88px; height: 25px; box-sizing: border-box; padding: 2px 5px; color: var(--bone-13); font: 11px Consolas, monospace; background: rgba(0, 0, 0, 0.35); border: 1px solid var(--ground-10); outline: none; }
  .message { grid-column: 2; grid-row: 1 / 3; align-self: start; }
  .message .field { width: 100%; }
  .sound { grid-template-columns: 70px minmax(90px, 1fr) minmax(65px, 0.8fr) 32px; }
  .sound small { grid-column: 4; text-align: right; }
  .sound input[type='range'] { min-width: 55px; }
  .alert-notes { grid-column: 1 / -1; display: flex; flex-direction: column; gap: 2px; padding-top: 4px; color: var(--dim-2); font: 8px Consolas, monospace; border-top: 1px solid rgba(62, 42, 44, 0.5); overflow-wrap: anywhere; }
  .overlap { color: #a890bd; }
  .row-actions { grid-column: 1 / -1; display: flex; align-items: center; justify-content: space-between; gap: 8px; }

  .field,
  .picker {
    box-sizing: border-box;
    height: 25px;
    min-width: 0;
    padding: 2px 6px;
    color: var(--bone-13);
    font: inherit;
    font-size: 10px;
    background: rgba(0, 0, 0, 0.35);
    border: 1px solid var(--ground-10);
    outline: none;
  }
  .field:focus,
  .field:hover,
  .picker:focus,
  .picker:hover { border-color: var(--edge-4); }
  .picker { width: 100%; appearance: none; cursor: pointer; }
  .picker option,
  .picker optgroup { color: var(--bone-9); background: var(--ground-7); }

  input[type='range'] {
    height: 14px;
    appearance: none;
    -webkit-appearance: none;
    background: none;
    cursor: pointer;
  }
  input[type='range']::-webkit-slider-runnable-track { height: 4px; background: var(--ground-7); border: 1px solid var(--ground-11); }
  input[type='range']::-webkit-slider-thumb { width: 11px; height: 11px; margin-top: -5px; -webkit-appearance: none; background: #a970ff; border: 1px solid #e1ccff; }
  input[type='range']:disabled { opacity: 0.35; }

  .btn {
    box-sizing: border-box;
    height: 25px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    padding: 0 7px;
    color: var(--bone-13);
    font: inherit;
    font-size: 10px;
    line-height: 1;
    border: 6px solid transparent;
    border-image-source: var(--btn);
    border-image-slice: 6 fill;
    border-image-width: 6px;
    image-rendering: pixelated;
    cursor: pointer;
  }
  .btn:hover:not(:disabled) { border-image-source: var(--btn-hover); }
  .btn:active:not(:disabled) { border-image-source: var(--btn-down); }
  .btn:disabled { opacity: 0.38; cursor: default; }
  .btn.purple { color: #f1e8ff; filter: hue-rotate(245deg) saturate(1.35); }
  .btn.danger { color: #ffb6b6; }
  .btn.mini { height: 22px; padding: 0 5px; font-size: 9px; }
  .btn.testing { filter: brightness(1.5) saturate(1.4); }
  .link { padding: 1px 0; color: var(--edge-2b); font-size: 9px; text-decoration: underline; }
  .link:hover { color: var(--bone-13); }

  .toast { position: sticky; bottom: 4px; align-self: center; max-width: min(520px, 90%); padding: 5px 9px; color: #e8dbff; font-size: 10px; background: rgba(38, 20, 48, 0.96); border: 1px solid #7551a0; box-shadow: 0 3px 14px black; z-index: 5; }
  .empty { padding: 20px; color: var(--dim-2); text-align: center; }

  @media (max-width: 900px) {
    .top-grid { grid-template-columns: minmax(0, 1fr); }
    .mast { align-items: flex-start; flex-direction: column; }
    details.alert > summary { grid-template-columns: 21px minmax(110px, 1fr) 82px 44px 13px; }
    .alert-desc { display: none; }
    .alert-body { grid-template-columns: minmax(0, 1fr); padding-left: 10px; }
    .message { grid-column: 1; grid-row: auto; }
    .alert-notes,
    .row-actions { grid-column: 1; }
  }
</style>
