// Twitch alert settings live in one JSON object in the normal HS Tracker
// settings file.  The catalog is the single source of truth for the UI and for
// scope calculation: enabling an alert asks for exactly the permission that
// alert needs, and nothing else.

export const TWITCH_CONFIG_VERSION = 1;
export const TWITCH_TEXT_LIMIT = 240;

export const TWITCH_ALERT_GROUPS = Object.freeze([
  { id: 'audience', label: 'Audience', description: 'The high-signal alerts most streams put on screen.' },
  { id: 'support', label: 'Support & rewards', description: 'Bits, Power-ups, redemptions and charity.' },
  { id: 'milestones', label: 'Milestones', description: 'Hype Trains, goals and interactive channel events.' },
  { id: 'broadcast', label: 'Broadcast', description: 'Stream state and channel operations.' },
  { id: 'chat', label: 'Chat moments', description: 'Filtered chat-notification moments without duplicate sub alerts.' },
  { id: 'optional', label: 'Optional / noisy', description: 'Useful for some shows; disabled until you opt in.' },
]);

const row = (kind, label, group, options = {}) => Object.freeze({
  kind,
  label,
  group,
  defaultEnabled: options.defaultEnabled ?? true,
  defaultThreshold: options.defaultThreshold ?? 1,
  thresholdUnit: options.thresholdUnit ?? 'event',
  defaultText: options.defaultText ?? `{user} triggered ${label}`,
  scopes: Object.freeze(options.scopes ?? []),
  eventsub: Object.freeze(options.eventsub ?? []),
  placeholders: Object.freeze(options.placeholders ?? ['user']),
  overlap: options.overlap ?? '',
  description: options.description ?? '',
});

// One logical alert can use several EventSub lifecycle messages. Conversely,
// several logical alerts can share channel.chat.notification or
// channel.bits.use. The backend subscribes once and dispatches by subtype.
export const TWITCH_ALERT_CATALOG = Object.freeze([
  row('follow', 'New follow', 'audience', {
    defaultText: '{user} joined the siege!',
    scopes: ['moderator:read:followers'],
    eventsub: ['channel.follow:2'],
    description: 'A viewer follows the channel.',
  }),
  row('new_sub', 'New subscription', 'audience', {
    defaultText: '{user} subscribed at tier {tier}!',
    scopes: ['channel:read:subscriptions'],
    eventsub: ['channel.subscribe:1'],
    placeholders: ['user', 'tier'],
    overlap: 'Ignore channel.subscribe when is_gift is true; sub_gift owns gifted subscriptions.',
    description: 'A paid, Prime or gifted-recipient subscription. Gift recipients are folded into the gift alert.',
  }),
  row('resub', 'Resub message', 'audience', {
    defaultText: '{user} resubscribed for {months} months!',
    scopes: ['channel:read:subscriptions'],
    eventsub: ['channel.subscription.message:1'],
    placeholders: ['user', 'months', 'message', 'tier'],
    overlap: 'Handled by channel.subscription.message, not the matching chat-notification subtype.',
    description: 'A subscriber shares their anniversary message.',
  }),
  row('sub_gift', 'Gift subscriptions', 'audience', {
    defaultText: '{user} gifted {count} subs!',
    thresholdUnit: 'gifts',
    scopes: ['channel:read:subscriptions'],
    eventsub: ['channel.subscription.gift:1'],
    placeholders: ['user', 'count', 'total', 'tier', 'anonymous'],
    overlap: 'Use the gift summary and suppress each is_gift recipient from channel.subscribe.',
    description: 'One summary for a single gift or a multi-sub gift batch.',
  }),
  row('raid', 'Incoming raid', 'audience', {
    defaultText: '{user} raided with {viewers} viewers!',
    defaultThreshold: 1,
    thresholdUnit: 'viewers',
    eventsub: ['channel.raid:1'],
    placeholders: ['user', 'viewers'],
    description: 'Another broadcaster raids into this channel.',
  }),

  row('bits', 'Bits / Cheer', 'support', {
    defaultText: '{user} cheered {bits} bits!',
    defaultThreshold: 1,
    thresholdUnit: 'bits',
    scopes: ['bits:read'],
    eventsub: ['channel.bits.use:1'],
    placeholders: ['user', 'bits', 'message', 'anonymous'],
    overlap: 'channel.bits.use is authoritative; do not also subscribe to channel.cheer.',
    description: 'A Cheer paid with Bits. Anonymous Cheers stay anonymous.',
  }),
  row('power_up', 'Power-up', 'support', {
    defaultText: '{user} used {power_up}!',
    defaultThreshold: 1,
    thresholdUnit: 'bits',
    scopes: ['bits:read'],
    eventsub: ['channel.bits.use:1'],
    placeholders: ['user', 'bits', 'power_up', 'message'],
    overlap: 'Shares one channel.bits.use subscription with Bits and dispatches only Power-up subtypes.',
    description: 'Message Effects, Gigantify an Emote and custom Power-ups.',
  }),
  row('channel_points', 'Custom channel reward', 'support', {
    defaultText: '{user} redeemed {reward}!',
    thresholdUnit: 'redemption',
    scopes: ['channel:read:redemptions'],
    eventsub: ['channel.channel_points_custom_reward_redemption.add:1'],
    placeholders: ['user', 'reward', 'cost', 'message'],
    description: 'A redemption for one of the channel’s custom rewards.',
  }),
  row('automatic_points', 'Automatic channel reward', 'support', {
    defaultText: '{user} redeemed {reward}!',
    thresholdUnit: 'redemption',
    scopes: ['channel:read:redemptions'],
    eventsub: ['channel.channel_points_automatic_reward_redemption.add:2'],
    placeholders: ['user', 'reward', 'cost', 'message'],
    description: 'Built-in rewards such as Highlight My Message or Unlock a Random Sub Emote.',
  }),
  row('charity_donation', 'Charity donation', 'support', {
    defaultText: '{user} donated {amount} to {charity}!',
    defaultThreshold: 1,
    thresholdUnit: 'currency units',
    scopes: ['channel:read:charity'],
    eventsub: ['channel.charity_campaign.donate:1'],
    placeholders: ['user', 'amount', 'currency', 'charity'],
    description: 'A donation made through Twitch Charity.',
  }),

  row('hype_train', 'Hype Train', 'milestones', {
    defaultText: 'Hype Train reached level {level}!',
    defaultThreshold: 1,
    thresholdUnit: 'level',
    scopes: ['channel:read:hype_train'],
    eventsub: ['channel.hype_train.begin:2', 'channel.hype_train.progress:2', 'channel.hype_train.end:2'],
    placeholders: ['level', 'progress', 'goal', 'top_user'],
    description: 'Begin, level-up/progress and completion moments; routine progress is coalesced by the backend.',
  }),
  row('goal', 'Creator goal', 'milestones', {
    defaultText: '{goal} reached {current}/{target}!',
    thresholdUnit: 'progress',
    scopes: ['channel:read:goals'],
    eventsub: ['channel.goal.begin:1', 'channel.goal.progress:1', 'channel.goal.end:1'],
    placeholders: ['goal', 'current', 'target', 'description'],
    description: 'Follower, subscription and related creator-goal milestones.',
  }),
  row('poll', 'Poll result', 'milestones', {
    defaultText: 'Poll finished: {winner} won with {votes} votes!',
    thresholdUnit: 'votes',
    scopes: ['channel:read:polls'],
    eventsub: ['channel.poll.begin:1', 'channel.poll.progress:1', 'channel.poll.end:1'],
    placeholders: ['title', 'winner', 'votes'],
    description: 'Poll opening, meaningful progress and the final result; repeated progress is coalesced.',
  }),
  row('prediction', 'Prediction result', 'milestones', {
    defaultText: 'Prediction settled: {winner}!',
    thresholdUnit: 'points wagered',
    scopes: ['channel:read:predictions'],
    eventsub: ['channel.prediction.begin:1', 'channel.prediction.progress:1', 'channel.prediction.lock:1', 'channel.prediction.end:1'],
    placeholders: ['title', 'winner', 'points', 'users'],
    description: 'Prediction opening, lock and resolution; repeated progress is coalesced.',
  }),
  row('shoutout', 'Shoutout', 'milestones', {
    defaultText: 'Go follow {user}: {title}',
    thresholdUnit: 'event',
    scopes: ['moderator:read:shoutouts'],
    eventsub: ['channel.shoutout.create:1', 'channel.shoutout.receive:1'],
    placeholders: ['user', 'title', 'viewers'],
    description: 'A shoutout sent or received by the channel.',
  }),

  row('stream_online', 'Stream online', 'broadcast', {
    defaultEnabled: false,
    defaultText: 'We are live: {title}',
    eventsub: ['stream.online:1'],
    placeholders: ['title', 'category'],
    description: 'The broadcast starts. Usually useful for a local test or automation rather than an on-stream overlay.',
  }),
  row('stream_offline', 'Stream offline', 'broadcast', {
    defaultEnabled: false,
    defaultText: 'Stream ended — thanks for watching!',
    eventsub: ['stream.offline:1'],
    placeholders: [],
    description: 'The broadcast stops.',
  }),
  row('ad_break', 'Ad break', 'broadcast', {
    defaultEnabled: false,
    defaultText: 'Ad break: {duration} seconds',
    defaultThreshold: 1,
    thresholdUnit: 'seconds',
    scopes: ['channel:read:ads'],
    eventsub: ['channel.ad_break.begin:1'],
    placeholders: ['duration', 'automatic'],
    description: 'An automatic or manually triggered ad break begins.',
  }),
  row('channel_update', 'Channel update', 'broadcast', {
    defaultEnabled: false,
    defaultText: 'Now playing {category}: {title}',
    eventsub: ['channel.update:2'],
    placeholders: ['category', 'title', 'language'],
    description: 'Title, category, language or content-label changes.',
  }),

  row('chat_announcement', 'Chat announcement', 'chat', {
    defaultText: '{user}: {message}',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'message', 'color'],
    overlap: 'Dispatch only announcement; dedicated subscriptions own sub, gift, resub and raid chat notifications.',
    description: 'A highlighted moderator announcement in chat.',
  }),
  row('watch_streak', 'Watch streak', 'chat', {
    defaultText: '{user} is on a {months}-month watch streak!',
    defaultThreshold: 1,
    thresholdUnit: 'months',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'months', 'message'],
    overlap: 'Dispatch only the watch_streak chat-notification subtype.',
    description: 'A viewer shares their watch streak in chat.',
  }),
  row('modiversary', 'Mod anniversary', 'chat', {
    defaultText: '{user} celebrates {months} months as a mod!',
    defaultThreshold: 1,
    thresholdUnit: 'months',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'months', 'message'],
    overlap: 'Dispatch only the modiversary chat-notification subtype.',
    description: 'A moderator shares their mod anniversary.',
  }),
  row('bits_badge', 'Bits badge milestone', 'chat', {
    defaultText: '{user} unlocked the {threshold} Bits badge!',
    defaultThreshold: 1,
    thresholdUnit: 'badge bits',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'threshold'],
    overlap: 'Dispatch only bits_badge_tier; it is not a new Cheer and must not also fire the Bits alert.',
    description: 'A chatter shares a newly unlocked Bits badge tier.',
  }),
  row('user_intro', 'First-time chatter intro', 'chat', {
    defaultText: 'Welcome to chat, {user}!',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'message'],
    overlap: 'Dispatch only user_intro.',
    description: 'A first-time chatter introduces themselves.',
  }),
  row('sub_upgrade', 'Gift sub upgrade', 'chat', {
    defaultEnabled: false,
    defaultText: '{user} upgraded their gifted sub!',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'gifter'],
    overlap: 'Dispatch only gift_paid_upgrade or prime_paid_upgrade.',
    description: 'A viewer continues a gift or converts Prime to a paid subscription.',
  }),
  row('pay_it_forward', 'Pay it forward', 'chat', {
    defaultEnabled: false,
    defaultText: '{user} paid {gifter}\'s gift forward!',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.notification:1'],
    placeholders: ['user', 'gifter'],
    overlap: 'Dispatch only pay_it_forward.',
    description: 'A gift recipient gives a subscription to another viewer.',
  }),

  row('outgoing_raid', 'Outgoing raid', 'optional', {
    defaultEnabled: false,
    defaultText: 'Raiding {user} with {viewers} viewers!',
    defaultThreshold: 1,
    thresholdUnit: 'viewers',
    eventsub: ['channel.raid:1'],
    placeholders: ['user', 'viewers'],
    overlap: 'Shares channel.raid with incoming raids; dispatch by from/to condition.',
    description: 'This channel sends its viewers to another broadcaster.',
  }),
  row('charity_campaign', 'Charity campaign lifecycle', 'optional', {
    defaultEnabled: false,
    defaultText: '{charity}: {current} raised toward {target}',
    thresholdUnit: 'currency units',
    scopes: ['channel:read:charity'],
    eventsub: ['channel.charity_campaign.start:1', 'channel.charity_campaign.progress:1', 'channel.charity_campaign.stop:1'],
    placeholders: ['charity', 'current', 'target', 'currency'],
    description: 'Campaign start, progress and stop events. Donation alerts remain separate.',
  }),
  row('shared_chat', 'Shared Chat activity', 'optional', {
    defaultEnabled: false,
    defaultText: '{user} in {channel}: {message}',
    scopes: ['user:read:chat'],
    eventsub: ['channel.chat.message:1'],
    placeholders: ['user', 'channel', 'message'],
    overlap: 'Dispatch only source_broadcaster messages; notification subtypes remain owned by the native alert rows above.',
    description: 'Messages arriving through a Shared Chat session. This can be very noisy.',
  }),
]);

export const TWITCH_ALERT_BY_KIND = new Map(TWITCH_ALERT_CATALOG.map((alert) => [alert.kind, alert]));

// Rust normalizes Twitch's many lifecycle payloads into precise event kinds.
// The settings UI intentionally aggregates those into the alert concepts a
// streamer configures. Keep that translation explicit so a progress event and
// its completion use the same saved style without disguising the raw event in
// diagnostics.
export const TWITCH_EVENT_KIND_TO_ALERT = Object.freeze({
  follow: 'follow',
  subscription: 'new_sub',
  resubscription: 'resub',
  gift_subscription: 'sub_gift',
  cheer: 'bits',
  power_up: 'power_up',
  raid: 'raid',
  outgoing_raid: 'outgoing_raid',
  reward_redemption: 'channel_points',
  automatic_reward: 'automatic_points',
  charity_donation: 'charity_donation',
  hype_train_begin: 'hype_train',
  hype_train_progress: 'hype_train',
  hype_train_end: 'hype_train',
  goal_begin: 'goal',
  goal_progress: 'goal',
  goal_end: 'goal',
  poll_begin: 'poll',
  poll_progress: 'poll',
  poll_end: 'poll',
  prediction_begin: 'prediction',
  prediction_progress: 'prediction',
  prediction_lock: 'prediction',
  prediction_end: 'prediction',
  charity_campaign_start: 'charity_campaign',
  charity_campaign_progress: 'charity_campaign',
  charity_campaign_stop: 'charity_campaign',
  shoutout_created: 'shoutout',
  shoutout_received: 'shoutout',
  stream_online: 'stream_online',
  stream_offline: 'stream_offline',
  ad_break: 'ad_break',
  channel_update: 'channel_update',
  chat_upgrade: 'sub_upgrade',
  pay_it_forward: 'pay_it_forward',
  watch_streak: 'watch_streak',
  modiversary: 'modiversary',
  bits_badge: 'bits_badge',
  announcement: 'chat_announcement',
  user_intro: 'user_intro',
  shared_chat: 'shared_chat',
});

export function logicalTwitchAlertKind(kind) {
  const value = String(kind ?? '');
  return TWITCH_EVENT_KIND_TO_ALERT[value] ?? (TWITCH_ALERT_BY_KIND.has(value) ? value : '');
}

export const TWITCH_SOUND_OPTIONS = Object.freeze([
  ['default', 'Twitch default'],
  ['none', 'Silent'],
  ['satanic', 'Satanic drop'],
  ['set', 'Set drop'],
  ['heroic', 'Heroic drop'],
  ['angelic', 'Angelic drop'],
  ['unholy', 'Unholy drop'],
  ['mail', 'Mail'],
  ['zone', 'Zone'],
]);

const SOUND_KEYS = new Set(TWITCH_SOUND_OPTIONS.map(([key]) => key));

function cleanText(value, fallback = '', limit = TWITCH_TEXT_LIMIT) {
  const text = String(value ?? '').replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, '').trim().slice(0, limit);
  return text || fallback;
}

function number(value, fallback, min, max) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.min(max, Math.max(min, parsed)) : fallback;
}

export function cleanTwitchClientId(value) {
  // Twitch client IDs are opaque ASCII. Reject whitespace and punctuation that
  // can only come from accidentally pasting a URL, token or secret.
  return String(value ?? '').trim().toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 64);
}

export function defaultTwitchAlert(alert) {
  return {
    enabled: alert.defaultEnabled,
    threshold: alert.defaultThreshold,
    text: alert.defaultText,
    fx_preset: 'current',
    sound: 'default',
    volume: 0.7,
  };
}

export function normaliseTwitchAlert(kind, value = {}) {
  const alert = TWITCH_ALERT_BY_KIND.get(kind);
  if (!alert) return null;
  const input = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  const defaults = defaultTwitchAlert(alert);
  return {
    enabled: typeof input.enabled === 'boolean' ? input.enabled : defaults.enabled,
    threshold: number(input.threshold, defaults.threshold, 0, 1_000_000_000),
    text: cleanText(input.text, defaults.text),
    fx_preset: cleanText(input.fx_preset, 'current', 96),
    sound: SOUND_KEYS.has(input.sound) ? input.sound : defaults.sound,
    volume: number(input.volume, defaults.volume, 0, 1),
  };
}

export function normaliseTwitchAlerts(value = {}) {
  const input = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  return Object.fromEntries(TWITCH_ALERT_CATALOG.map((alert) => [alert.kind, normaliseTwitchAlert(alert.kind, input[alert.kind])]));
}

export function requiredTwitchScopes(value = {}) {
  const alerts = normaliseTwitchAlerts(value);
  const scopes = new Set();
  for (const alert of TWITCH_ALERT_CATALOG) {
    if (!alerts[alert.kind].enabled) continue;
    for (const scope of alert.scopes) scopes.add(scope);
  }
  return [...scopes].sort();
}

export function twitchEventSubPlan(value = {}) {
  const alerts = normaliseTwitchAlerts(value);
  const subscriptions = new Map();
  for (const alert of TWITCH_ALERT_CATALOG) {
    if (!alerts[alert.kind].enabled) continue;
    for (const signature of alert.eventsub) {
      const kinds = subscriptions.get(signature) ?? [];
      kinds.push(alert.kind);
      subscriptions.set(signature, kinds);
    }
  }
  return [...subscriptions].sort(([left], [right]) => left.localeCompare(right)).map(([signature, kinds]) => ({
    signature,
    kinds,
  }));
}

function canonicalJsonValue(value) {
  if (Array.isArray(value)) return value.map(canonicalJsonValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value).sort().map((key) => [key, canonicalJsonValue(value[key])]),
    );
  }
  return value;
}

export function sameTwitchSettingsSnapshot(left, right) {
  if (!left || !right) return false;
  try {
    return JSON.stringify(canonicalJsonValue(left)) === JSON.stringify(canonicalJsonValue(right));
  } catch {
    return false;
  }
}
