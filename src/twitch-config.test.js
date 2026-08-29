import test from 'node:test';
import assert from 'node:assert/strict';

import {
  TWITCH_ALERT_CATALOG,
  cleanTwitchClientId,
  logicalTwitchAlertKind,
  normaliseTwitchAlerts,
  requiredTwitchScopes,
  sameTwitchSettingsSnapshot,
  twitchEventSubPlan,
} from './twitch-config.js';

test('catalog contains every native alert exposed by the Twitch tab', () => {
  const kinds = new Set(TWITCH_ALERT_CATALOG.map((alert) => alert.kind));
  for (const kind of [
    'follow', 'new_sub', 'resub', 'sub_gift', 'bits', 'power_up', 'raid',
    'channel_points', 'automatic_points', 'charity_donation', 'hype_train',
    'goal', 'poll', 'prediction', 'shoutout', 'stream_online', 'stream_offline',
    'ad_break', 'channel_update', 'chat_announcement', 'watch_streak',
    'modiversary', 'bits_badge', 'user_intro', 'outgoing_raid',
    'charity_campaign', 'shared_chat',
  ]) assert.equal(kinds.has(kind), true, `${kind} is missing`);
  assert.equal(kinds.size, TWITCH_ALERT_CATALOG.length, 'alert kinds must be unique');
});

test('normalisation fills the catalog, clamps values and drops unknown keys', () => {
  const alerts = normaliseTwitchAlerts({
    bits: { enabled: false, threshold: -9, text: '', fx_preset: 'frost', sound: 'bogus', volume: 5 },
    raid: { threshold: '25', text: '{user} brought the party' },
    not_a_twitch_event: { enabled: true },
  });

  assert.equal(Object.keys(alerts).length, TWITCH_ALERT_CATALOG.length);
  assert.deepEqual(Object.hasOwn(alerts, 'not_a_twitch_event'), false);
  assert.equal(alerts.bits.enabled, false);
  assert.equal(alerts.bits.threshold, 0);
  assert.equal(alerts.bits.text.includes('cheered'), true);
  assert.equal(alerts.bits.fx_preset, 'frost');
  assert.equal(alerts.bits.sound, 'default');
  assert.equal(alerts.bits.volume, 1);
  assert.equal(alerts.raid.threshold, 25);
  assert.equal(alerts.raid.text, '{user} brought the party');
});

test('scope calculation is least-privilege and reflects enabled alerts', () => {
  const disabled = Object.fromEntries(TWITCH_ALERT_CATALOG.map(({ kind }) => [kind, { enabled: false }]));
  assert.deepEqual(requiredTwitchScopes(disabled), []);

  disabled.follow.enabled = true;
  disabled.bits.enabled = true;
  disabled.power_up.enabled = true;
  disabled.poll.enabled = true;
  assert.deepEqual(requiredTwitchScopes(disabled), [
    'bits:read',
    'channel:read:polls',
    'moderator:read:followers',
  ]);
});

test('shared EventSub sources are subscribed once and dispatched to logical alerts', () => {
  const disabled = Object.fromEntries(TWITCH_ALERT_CATALOG.map(({ kind }) => [kind, { enabled: false }]));
  disabled.bits.enabled = true;
  disabled.power_up.enabled = true;
  disabled.resub.enabled = true;
  const plan = twitchEventSubPlan(disabled);
  assert.deepEqual(plan.find(({ signature }) => signature === 'channel.bits.use:1')?.kinds, ['bits', 'power_up']);
  assert.equal(plan.filter(({ signature }) => signature === 'channel.bits.use:1').length, 1);
  assert.equal(plan.some(({ signature }) => signature === 'channel.cheer:1'), false);
});

test('normalized backend lifecycle events resolve to the configured aggregate', () => {
  assert.equal(logicalTwitchAlertKind('gift_subscription'), 'sub_gift');
  assert.equal(logicalTwitchAlertKind('hype_train_progress'), 'hype_train');
  assert.equal(logicalTwitchAlertKind('prediction_lock'), 'prediction');
  assert.equal(logicalTwitchAlertKind('outgoing_raid'), 'outgoing_raid');
  assert.equal(logicalTwitchAlertKind('shared_chat'), 'shared_chat');
  assert.equal(logicalTwitchAlertKind('new_sub'), 'new_sub');
  assert.equal(logicalTwitchAlertKind('unknown_event'), '');
});

test('shared chat uses message EventSub without duplicating native notifications', () => {
  const shared = TWITCH_ALERT_CATALOG.find(({ kind }) => kind === 'shared_chat');
  assert.deepEqual(shared.eventsub, ['channel.chat.message:1']);
});

test('client ID cleaner cannot mistake a URL or copied punctuation for the ID', () => {
  assert.equal(cleanTwitchClientId('  AbC123  '), 'abc123');
  assert.equal(cleanTwitchClientId('abc-123 /?'), 'abc123');
});

test('settings echo comparison ignores JSON map order but not value changes', () => {
  const sent = { twitch_alerts: { follow: { enabled: true }, bits: { threshold: 100 } }, scale: 1 };
  const rustEcho = { scale: 1, twitch_alerts: { bits: { threshold: 100 }, follow: { enabled: true } } };
  const changed = { scale: 1, twitch_alerts: { bits: { threshold: 200 }, follow: { enabled: true } } };
  assert.equal(sameTwitchSettingsSnapshot(sent, rustEcho), true);
  assert.equal(sameTwitchSettingsSnapshot(sent, changed), false);
});
