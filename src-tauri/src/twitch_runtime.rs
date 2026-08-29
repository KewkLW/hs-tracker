//! Tauri-facing Twitch coordinator.
//!
//! `twitch` owns protocol details and deliberately knows nothing about Tauri.
//! This layer owns settings, the OS credential vault, command state and the
//! handoff into HS Tracker's existing sound/flourish queues.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::twitch::{
    self, DeviceAuthorization, DeviceTokenPoll, EventSubHandle, TokenPersistence, TwitchAlert,
    TwitchClient, TwitchConnectionState, TwitchError, TwitchEventSubConfig, TwitchServiceEvent,
    TwitchStatus,
};
use crate::Settings;

const VAULT_SERVICE: &str = "com.hstracker.app.twitch";

#[derive(Clone)]
struct VaultTokenStore {
    account: String,
}

impl VaultTokenStore {
    fn new(client_id: &str) -> Self {
        Self {
            account: format!("oauth:{client_id}"),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, TwitchError> {
        keyring::Entry::new(VAULT_SERVICE, &self.account).map_err(vault_error)
    }
}

fn vault_error(error: keyring::Error) -> TwitchError {
    TwitchError::Persistence(format!(
        "the operating-system credential vault is unavailable: {error}"
    ))
}

impl TokenPersistence for VaultTokenStore {
    fn load(&self) -> Result<Option<Vec<u8>>, TwitchError> {
        match self.entry()?.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(vault_error(error)),
        }
    }

    fn save(&self, bytes: &[u8]) -> Result<(), TwitchError> {
        self.entry()?.set_secret(bytes).map_err(vault_error)
    }

    fn clear(&self) -> Result<(), TwitchError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(vault_error(error)),
        }
    }
}

pub struct TwitchRuntime {
    inner: Mutex<RuntimeInner>,
}

struct RuntimeInner {
    debug_mode: bool,
    enabled: bool,
    client_id: String,
    subscriptions: Vec<String>,
    alerts: Value,
    client: Option<TwitchClient>,
    authorization: Option<DeviceAuthorization>,
    service: Option<EventSubHandle>,
    generation: u64,
    status: TwitchStatus,
    recent_progress: BTreeMap<String, Instant>,
}

impl Default for TwitchRuntime {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                debug_mode: false,
                enabled: false,
                client_id: String::new(),
                subscriptions: Vec::new(),
                alerts: json!({}),
                client: None,
                authorization: None,
                service: None,
                generation: 0,
                status: TwitchStatus::default(),
                recent_progress: BTreeMap::new(),
            }),
        }
    }
}

fn make_client(client_id: &str) -> Result<TwitchClient, TwitchError> {
    TwitchClient::new(
        client_id,
        Arc::new(VaultTokenStore::new(client_id)) as Arc<dyn TokenPersistence>,
    )
}

fn stop_service(inner: &mut RuntimeInner) {
    if let Some(service) = inner.service.take() {
        service.request_stop();
        // Dropping detaches rather than blocking the UI on a socket timeout.
        // Its generation is retired below, so a last status cannot win.
        drop(service);
    }
}

fn start_service(app: &AppHandle, inner: &mut RuntimeInner) -> Result<(), TwitchError> {
    let client = inner.client.clone().ok_or_else(|| {
        TwitchError::InvalidConfiguration("enter a public Twitch Client ID".into())
    })?;
    stop_service(inner);
    inner.generation = inner.generation.wrapping_add(1);
    let generation = inner.generation;
    let app_for_events = app.clone();
    let sink = Arc::new(move |event| service_event(&app_for_events, generation, event));
    let config = TwitchEventSubConfig {
        broadcaster_user_id: String::new(),
        enabled_subscriptions: inner.subscriptions.clone(),
    };
    inner.status.state = TwitchConnectionState::Connecting;
    inner.status.connected = false;
    inner.status.websocket_state = "connecting".into();
    inner.status.error = None;
    inner.service = Some(twitch::spawn_eventsub_service(client, config, sink));
    Ok(())
}

fn twitch_runtime_enabled(settings: &Settings) -> bool {
    settings.debug_mode && settings.twitch_enabled
}

/// Apply only the non-secret Twitch fields from Settings. Presentation changes
/// take effect immediately; the socket restarts only when its desired EventSub
/// subscription set actually changed.
pub fn configure(app: &AppHandle, settings: &Settings) {
    let client_id = settings.twitch_client_id.trim().to_ascii_lowercase();
    let subscriptions = subscription_keys(&settings.twitch_alerts);
    let enabled = twitch_runtime_enabled(settings);
    let mut emit = None;

    if let Ok(mut inner) = app.state::<TwitchRuntime>().inner.lock() {
        inner.alerts = settings.twitch_alerts.clone();
        let connection_changed = inner.debug_mode != settings.debug_mode
            || inner.enabled != enabled
            || inner.client_id != client_id
            || inner.subscriptions != subscriptions;
        if !connection_changed {
            return;
        }

        stop_service(&mut inner);
        inner.generation = inner.generation.wrapping_add(1);
        let client_changed = inner.client_id != client_id;
        inner.debug_mode = settings.debug_mode;
        inner.enabled = enabled;
        inner.subscriptions = subscriptions;
        if client_changed {
            inner.client_id = client_id.clone();
            inner.client = None;
            inner.authorization = None;
            inner.status = TwitchStatus::default();
        }

        if !inner.enabled {
            inner.status.state = TwitchConnectionState::Stopped;
            inner.status.connected = false;
            inner.status.websocket_state = "stopped".into();
            inner.status.error = None;
            emit = Some(inner.status.clone());
        } else if client_id.is_empty() {
            inner.status.state = TwitchConnectionState::AuthorizationRequired;
            inner.status.connected = false;
            inner.status.websocket_state = "authorization_required".into();
            inner.status.error =
                Some("Enter the Client ID from a Public Twitch application".into());
            emit = Some(inner.status.clone());
        } else {
            if inner.client.is_none() {
                match make_client(&client_id) {
                    Ok(client) => inner.client = Some(client),
                    Err(error) => {
                        inner.status.state = TwitchConnectionState::Error;
                        inner.status.websocket_state = "error".into();
                        inner.status.error = Some(error.to_string());
                        emit = Some(inner.status.clone());
                    }
                }
            }
            if emit.is_none() {
                let authorized = inner
                    .client
                    .as_ref()
                    .map(TwitchClient::has_authorization)
                    .transpose();
                match authorized {
                    Ok(Some(true)) => {
                        if let Err(error) = start_service(app, &mut inner) {
                            inner.status.state = TwitchConnectionState::Error;
                            inner.status.websocket_state = "error".into();
                            inner.status.error = Some(error.to_string());
                        }
                    }
                    Ok(Some(false)) | Ok(None) => {
                        inner.status.state = TwitchConnectionState::AuthorizationRequired;
                        inner.status.connected = false;
                        inner.status.websocket_state = "authorization_required".into();
                        inner.status.error = None;
                    }
                    Err(error) => {
                        inner.status.state = TwitchConnectionState::Error;
                        inner.status.websocket_state = "error".into();
                        inner.status.error = Some(error.to_string());
                    }
                }
                emit = Some(inner.status.clone());
            }
        }
    }
    if let Some(status) = emit {
        let _ = app.emit("twitch-status", status);
    }
}

pub fn shutdown(app: &AppHandle) {
    if let Ok(mut inner) = app.state::<TwitchRuntime>().inner.lock() {
        stop_service(&mut inner);
        inner.generation = inner.generation.wrapping_add(1);
    }
}

fn service_event(app: &AppHandle, generation: u64, event: TwitchServiceEvent) {
    match event {
        TwitchServiceEvent::Status(status) => {
            let accepted = if let Ok(mut inner) = app.state::<TwitchRuntime>().inner.lock() {
                if inner.generation != generation {
                    false
                } else {
                    inner.status = status.clone();
                    true
                }
            } else {
                false
            };
            if accepted {
                let _ = app.emit("twitch-status", status);
            }
        }
        TwitchServiceEvent::Alert(alert) => {
            let current = app
                .state::<TwitchRuntime>()
                .inner
                .lock()
                .map(|inner| inner.generation == generation && inner.enabled)
                .unwrap_or(false);
            if current {
                let _ = app.emit("twitch-alert", &alert);
                dispatch_alert(app, &alert, false);
            }
        }
        TwitchServiceEvent::SubscriptionError {
            key,
            event_type,
            message,
        } => {
            service_notice(app, generation, format!("{key} ({event_type}): {message}"));
        }
        TwitchServiceEvent::SubscriptionRevoked {
            event_type,
            status,
            reason,
        } => {
            service_notice(app, generation, format!("{event_type} {status}: {reason}"));
        }
    }
}

fn service_notice(app: &AppHandle, generation: u64, message: String) {
    let status = if let Ok(mut inner) = app.state::<TwitchRuntime>().inner.lock() {
        if inner.generation != generation {
            return;
        }
        inner.status.error = Some(message);
        inner.status.clone()
    } else {
        return;
    };
    let _ = app.emit("twitch-status", status);
}

#[derive(Clone)]
struct AlertRule {
    enabled: bool,
    threshold: f64,
    text: String,
    fx_preset: String,
    sound: String,
    volume: f64,
}

fn rule(alerts: &Value, key: &str) -> AlertRule {
    let value = alerts.get(key).unwrap_or(&Value::Null);
    AlertRule {
        enabled: value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| default_enabled(key)),
        threshold: value
            .get("threshold")
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
            .clamp(0.0, 1_000_000_000.0),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("{user} triggered {headline}")
            .chars()
            .take(240)
            .collect(),
        fx_preset: value
            .get("fx_preset")
            .and_then(Value::as_str)
            .unwrap_or("current")
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            .take(96)
            .collect(),
        sound: match value
            .get("sound")
            .and_then(Value::as_str)
            .unwrap_or("default")
        {
            sound @ ("default" | "none" | "satanic" | "set" | "heroic" | "angelic" | "unholy"
            | "mail" | "zone") => sound.into(),
            _ => "default".into(),
        },
        volume: value
            .get("volume")
            .and_then(Value::as_f64)
            .unwrap_or(0.7)
            .clamp(0.0, 1.0),
    }
}

fn default_enabled(key: &str) -> bool {
    !matches!(
        key,
        "stream_online"
            | "stream_offline"
            | "ad_break"
            | "channel_update"
            | "sub_upgrade"
            | "pay_it_forward"
            | "outgoing_raid"
            | "charity_campaign"
            | "shared_chat"
    )
}

fn subscription_keys(alerts: &Value) -> Vec<String> {
    const MAP: &[(&str, &[&str])] = &[
        ("follow", &["follow"]),
        ("new_sub", &["subscription"]),
        ("resub", &["resubscription"]),
        ("sub_gift", &["gift_subscription"]),
        ("bits", &["bits"]),
        ("power_up", &["bits"]),
        ("raid", &["raid"]),
        ("outgoing_raid", &["outgoing_raid"]),
        ("channel_points", &["custom_reward"]),
        ("automatic_points", &["automatic_reward"]),
        ("charity_donation", &["charity_donation"]),
        (
            "hype_train",
            &["hype_train_begin", "hype_train_progress", "hype_train_end"],
        ),
        ("goal", &["goal_begin", "goal_progress", "goal_end"]),
        ("poll", &["poll_begin", "poll_progress", "poll_end"]),
        (
            "prediction",
            &[
                "prediction_begin",
                "prediction_progress",
                "prediction_lock",
                "prediction_end",
            ],
        ),
        ("shoutout", &["shoutout_created", "shoutout_received"]),
        ("stream_online", &["stream_online"]),
        ("stream_offline", &["stream_offline"]),
        ("ad_break", &["ad_break"]),
        ("channel_update", &["channel_update"]),
        (
            "charity_campaign",
            &["charity_start", "charity_progress", "charity_stop"],
        ),
        ("sub_upgrade", &["chat_milestones"]),
        ("pay_it_forward", &["chat_milestones"]),
        ("chat_announcement", &["chat_milestones"]),
        ("watch_streak", &["chat_milestones"]),
        ("modiversary", &["chat_milestones"]),
        ("bits_badge", &["chat_milestones"]),
        ("user_intro", &["chat_milestones"]),
        ("shared_chat", &["shared_chat"]),
    ];
    let mut selected = BTreeSet::new();
    for (logical, subscriptions) in MAP {
        if rule(alerts, logical).enabled {
            selected.extend(subscriptions.iter().map(|key| (*key).to_owned()));
        }
    }
    selected.into_iter().collect()
}

fn dispatch_alert(app: &AppHandle, alert: &TwitchAlert, force: bool) {
    let key = twitch::logical_alert_key(alert.kind);
    let picked = if let Ok(mut inner) = app.state::<TwitchRuntime>().inner.lock() {
        let configured = rule(&inner.alerts, key);
        if !force && (!inner.enabled || !configured.enabled) {
            return;
        }
        if !force && alert.source_type.ends_with(".progress") {
            let now = Instant::now();
            if inner
                .recent_progress
                .get(key)
                .is_some_and(|last| now.duration_since(*last) < Duration::from_secs(5))
            {
                return;
            }
            inner.recent_progress.insert(key.into(), now);
        }
        configured
    } else {
        return;
    };

    if !force && alert_metric(alert) < picked.threshold {
        return;
    }
    let actor = clip(
        if alert.anonymous {
            "Anonymous"
        } else {
            alert
                .user_name
                .as_deref()
                .or(alert.user_login.as_deref())
                .unwrap_or("Someone")
        },
        64,
    );
    let detail = clip(&render_template(&picked.text, alert, &actor), 240);
    let message = alert.message.as_deref().map(|message| clip(message, 180));
    let color = alert_color(key);
    let priority = alert_priority(key);
    let mut payload = json!({
        "kind": "twitch",
        "event": key,
        "actor": actor,
        "headline": clip(&alert.title, 80),
        "detail": detail,
        "amount": alert.amount,
        "currency": alert.currency,
        "count": alert.count,
        "tier": alert.tier,
        "priority": priority,
        "color": color,
        "fx_preset": picked.fx_preset,
    });
    if let Some(message) = message {
        payload["message"] = Value::String(message);
    }
    if matches!(key, "raid" | "outgoing_raid") {
        payload["viewers"] = json!(alert.count.unwrap_or(0));
    }

    let _ = app.emit(
        "twitch-sound",
        json!({ "event": key, "sound": picked.sound, "volume": picked.volume }),
    );
    show_twitch_flourish(app, payload);
}

fn show_twitch_flourish(app: &AppHandle, payload: Value) {
    if !crate::overlay_supported() {
        return;
    }
    let scale = crate::read_settings().flourish_scale.clamp(0.5, 2.0) as f64;
    crate::ensure_flourish(app, true, scale);
    if crate::emit_flourish_preview(app, &payload) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..100 {
            if crate::emit_flourish_preview(&app, &payload) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        crate::log::error("a Twitch alert timed out waiting for the flourish window");
    });
}

fn alert_metric(alert: &TwitchAlert) -> f64 {
    match twitch::logical_alert_key(alert.kind) {
        // A Hype Train's contribution total can be much larger than its level,
        // but the dashboard threshold is explicitly expressed in levels.
        "hype_train" => {
            return alert
                .details
                .get("level")
                .and_then(number_value)
                .or_else(|| alert.count.map(|count| count as f64))
                .unwrap_or(1.0);
        }
        // Poll thresholds and the {votes} placeholder both mean the leading
        // choice's votes, calculated by the protocol normalizer.
        "poll" => {
            return alert
                .details
                .get("votes")
                .and_then(number_value)
                .unwrap_or(1.0);
        }
        "watch_streak" | "modiversary" | "bits_badge" => {
            return alert.count.unwrap_or(1) as f64;
        }
        _ => {}
    }
    if let Some(amount) = alert.amount {
        return amount;
    }
    if let Some(count) = alert.count {
        return count as f64;
    }
    for key in [
        "level",
        "progress",
        "current",
        "reward_cost",
        "duration_seconds",
    ] {
        if let Some(value) = alert.details.get(key).and_then(number_value) {
            return value;
        }
    }
    1.0
}

fn number_value(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn render_template(template: &str, alert: &TwitchAlert, actor: &str) -> String {
    let amount = alert.amount.map(format_number).unwrap_or_default();
    let count = alert
        .count
        .map(|value| value.to_string())
        .unwrap_or_default();
    let detail = |key: &str| alert.details.get(key).map(value_text).unwrap_or_default();
    let winner = winner(alert);
    let replacements = [
        ("user", actor.to_owned()),
        ("headline", alert.title.clone()),
        ("message", alert.message.clone().unwrap_or_default()),
        ("amount", amount.clone()),
        ("bits", amount),
        ("currency", alert.currency.clone().unwrap_or_default()),
        ("count", count.clone()),
        ("viewers", count.clone()),
        ("months", count),
        ("tier", alert.tier.clone().unwrap_or_default()),
        (
            "reward",
            nonempty(
                detail("reward_title"),
                alert.message.clone().unwrap_or_default(),
            ),
        ),
        ("cost", detail("reward_cost")),
        (
            "level",
            nonempty(
                detail("level"),
                alert.count.map(|v| v.to_string()).unwrap_or_default(),
            ),
        ),
        ("progress", detail("progress")),
        ("goal", detail("goal")),
        ("current", detail("current")),
        ("target", detail("target")),
        (
            "title",
            nonempty(detail("title"), alert.message.clone().unwrap_or_default()),
        ),
        ("category", detail("category_name")),
        ("language", detail("language")),
        ("duration", detail("duration_seconds")),
        ("automatic", detail("is_automatic")),
        ("winner", winner),
        ("votes", detail("votes")),
        ("points", detail("points")),
        ("users", detail("users")),
        ("charity", alert.message.clone().unwrap_or_default()),
        (
            "gifter",
            alert.secondary_user_name.clone().unwrap_or_default(),
        ),
        ("channel", detail("source_broadcaster_user_name")),
        (
            "power_up",
            nonempty(detail("type"), alert.message.clone().unwrap_or_default()),
        ),
        ("threshold", amount_or_count(alert)),
        ("total", detail("total")),
        (
            "anonymous",
            if alert.anonymous {
                "anonymous".into()
            } else {
                String::new()
            },
        ),
        ("description", alert.message.clone().unwrap_or_default()),
        ("top_user", detail("top_user")),
    ];
    let mut rendered = template.to_owned();
    for (name, value) in replacements {
        rendered = rendered.replace(&format!("{{{name}}}"), &value);
    }
    rendered
}

fn winner(alert: &TwitchAlert) -> String {
    if let (Some(outcomes), Some(id)) = (
        alert.details.get("outcomes").and_then(Value::as_array),
        alert
            .details
            .get("winning_outcome_id")
            .and_then(Value::as_str),
    ) {
        if let Some(title) = outcomes
            .iter()
            .find(|outcome| outcome.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|outcome| outcome.get("title"))
            .and_then(Value::as_str)
        {
            return title.into();
        }
    }
    alert
        .details
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| {
            choices
                .iter()
                .max_by_key(|choice| poll_choice_votes(choice))
        })
        .and_then(|choice| choice.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into()
}

fn poll_choice_votes(choice: &Value) -> u64 {
    choice
        .get("votes")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            choice
                .get("channel_points_votes")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_add(
                    choice
                        .get("bits_votes")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                )
        })
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_bool().map(|value| value.to_string()))
        .or_else(|| value.as_f64().map(format_number))
        .unwrap_or_default()
}

fn amount_or_count(alert: &TwitchAlert) -> String {
    alert
        .amount
        .map(format_number)
        .or_else(|| alert.count.map(|value| value.to_string()))
        .unwrap_or_default()
}

fn format_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .into()
    }
}

fn nonempty(first: String, fallback: String) -> String {
    if first.is_empty() {
        fallback
    } else {
        first
    }
}

fn clip(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

fn alert_priority(key: &str) -> i64 {
    match key {
        "raid" | "sub_gift" | "charity_donation" | "hype_train" => 4,
        "new_sub" | "resub" | "bits" | "power_up" => 3,
        _ => 1,
    }
}

fn alert_color(key: &str) -> &'static str {
    match key {
        "raid" | "outgoing_raid" => "#f05ad7",
        "bits" | "power_up" | "sub_gift" | "new_sub" | "resub" => "#f0c75e",
        "channel_points" | "automatic_points" => "#62d6c4",
        "charity_donation" | "charity_campaign" => "#58d68d",
        "hype_train" => "#ff7a45",
        _ => "#a970ff",
    }
}

fn require_debug_mode(debug_mode: bool) -> Result<(), String> {
    if debug_mode {
        Ok(())
    } else {
        Err("Twitch alerts are available only while Debug Mode is enabled".into())
    }
}

fn app_debug_mode(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<TwitchRuntime>();
    let inner = state
        .inner
        .lock()
        .map_err(|_| "Twitch state is unavailable")?;
    require_debug_mode(inner.debug_mode)
}

#[tauri::command]
pub fn twitch_status(state: tauri::State<'_, TwitchRuntime>) -> Value {
    let status = state
        .inner
        .lock()
        .map(|inner| inner.status.clone())
        .unwrap_or_default();
    let mut value = serde_json::to_value(&status).unwrap_or_else(|_| json!({ "state": "error" }));
    value["authenticated"] = Value::Bool(status.display_name.is_some());
    value
}

#[tauri::command(async)]
pub fn twitch_begin_auth(
    app: AppHandle,
    client_id: String,
    scopes: Vec<String>,
) -> Result<DeviceAuthorization, String> {
    app_debug_mode(&app)?;
    let allowed: BTreeSet<String> = twitch::all_supported_scopes().into_iter().collect();
    let scopes = scopes
        .into_iter()
        .filter(|scope| allowed.contains(scope))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let client = make_client(client_id.trim()).map_err(|error| error.to_string())?;
    let authorization = client
        .begin_device_authorization(&scopes)
        .map_err(|error| error.to_string())?;
    let status = {
        let state = app.state::<TwitchRuntime>();
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "Twitch state is unavailable")?;
        if inner.client_id != client.client_id() {
            stop_service(&mut inner);
            inner.generation = inner.generation.wrapping_add(1);
            inner.client_id = client.client_id().into();
            inner.client = Some(client);
        } else if inner.client.is_none() {
            inner.client = Some(client);
        }
        inner.authorization = Some(authorization.clone());
        inner.status.state = TwitchConnectionState::AuthorizationRequired;
        inner.status.connected = false;
        inner.status.websocket_state = "awaiting_authorization".into();
        inner.status.error = None;
        inner.status.clone()
    };
    let _ = app.emit("twitch-status", status);
    Ok(authorization)
}

#[tauri::command(async)]
pub fn twitch_poll_auth(app: AppHandle) -> Result<Value, String> {
    let (client, authorization, generation) = {
        let state = app.state::<TwitchRuntime>();
        let inner = state
            .inner
            .lock()
            .map_err(|_| "Twitch state is unavailable")?;
        require_debug_mode(inner.debug_mode)?;
        (
            inner
                .client
                .clone()
                .ok_or("Start Twitch authorization first")?,
            inner
                .authorization
                .clone()
                .ok_or("Start Twitch authorization first")?,
            inner.generation,
        )
    };
    let polled = client
        .poll_device_authorization(&authorization)
        .map_err(|error| error.to_string())?;
    match polled {
        DeviceTokenPoll::Authorized { identity } => {
            let status = {
                let state = app.state::<TwitchRuntime>();
                let mut inner = state
                    .inner
                    .lock()
                    .map_err(|_| "Twitch state is unavailable")?;
                if inner.generation != generation || inner.client_id != client.client_id() {
                    return Err("That Twitch authorization was replaced by a newer one".into());
                }
                inner.authorization = None;
                inner.status.display_name = Some(identity.login);
                inner.status.granted_scopes = identity.scopes;
                inner.status.last_validation_at = Some(twitch::current_unix_millis());
                inner.status.error = None;
                if inner.enabled {
                    start_service(&app, &mut inner).map_err(|error| error.to_string())?;
                } else {
                    inner.status.state = TwitchConnectionState::Stopped;
                    inner.status.websocket_state = "stopped".into();
                }
                inner.status.clone()
            };
            let _ = app.emit("twitch-status", &status);
            Ok(json!({ "state": "connected", "status": status }))
        }
        DeviceTokenPoll::Pending {
            retry_after_seconds,
        } => Ok(json!({ "state": "pending", "retry_after_seconds": retry_after_seconds })),
        DeviceTokenPoll::SlowDown {
            retry_after_seconds,
        } => Ok(json!({ "state": "slow_down", "retry_after_seconds": retry_after_seconds })),
        DeviceTokenPoll::Denied => {
            clear_authorization(&app, "Twitch authorization was denied");
            Ok(json!({ "state": "denied" }))
        }
        DeviceTokenPoll::Expired => {
            clear_authorization(&app, "The Twitch activation code expired");
            Ok(json!({ "state": "expired" }))
        }
    }
}

fn clear_authorization(app: &AppHandle, message: &str) {
    let status = if let Ok(mut inner) = app.state::<TwitchRuntime>().inner.lock() {
        inner.authorization = None;
        inner.status.state = TwitchConnectionState::AuthorizationRequired;
        inner.status.websocket_state = "authorization_required".into();
        inner.status.error = Some(message.into());
        Some(inner.status.clone())
    } else {
        None
    };
    if let Some(status) = status {
        let _ = app.emit("twitch-status", status);
    }
}

#[tauri::command(async)]
pub fn twitch_disconnect(app: AppHandle) -> Result<(), String> {
    let (client, status) = {
        let state = app.state::<TwitchRuntime>();
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "Twitch state is unavailable")?;
        stop_service(&mut inner);
        inner.generation = inner.generation.wrapping_add(1);
        inner.authorization = None;
        let client = inner.client.clone();
        inner.status = TwitchStatus::default();
        (client, inner.status.clone())
    };
    if let Some(client) = client {
        client.disconnect().map_err(|error| error.to_string())?;
    }
    let _ = app.emit("twitch-status", status);
    Ok(())
}

#[tauri::command(async)]
pub fn twitch_restart(app: AppHandle) -> Result<(), String> {
    let status = {
        let state = app.state::<TwitchRuntime>();
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "Twitch state is unavailable")?;
        require_debug_mode(inner.debug_mode)?;
        if !inner.enabled {
            return Err("Enable the Twitch alert engine first".into());
        }
        start_service(&app, &mut inner).map_err(|error| error.to_string())?;
        inner.status.clone()
    };
    let _ = app.emit("twitch-status", status);
    Ok(())
}

#[tauri::command]
pub fn twitch_test_alert(app: AppHandle, kind: String) -> Result<(), String> {
    app_debug_mode(&app)?;
    let alert = twitch::sample_alert_for_test(&kind)
        .ok_or_else(|| "unknown Twitch alert type".to_owned())?;
    let _ = app.emit("twitch-alert", &alert);
    dispatch_alert(&app, &alert, true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twitch_runtime_and_mutating_commands_require_debug_mode() {
        assert!(require_debug_mode(false).is_err());
        assert!(require_debug_mode(true).is_ok());

        let mut settings = Settings {
            twitch_enabled: true,
            ..Settings::default()
        };
        assert!(
            !twitch_runtime_enabled(&settings),
            "an old enabled preference cannot connect after Debug Mode defaults off"
        );
        assert!(settings.twitch_enabled, "the saved preference is retained");
        settings.debug_mode = true;
        assert!(twitch_runtime_enabled(&settings));
        settings.twitch_enabled = false;
        assert!(!twitch_runtime_enabled(&settings));
    }

    #[test]
    fn subscription_plan_deduplicates_shared_eventsub_sources() {
        let alerts = json!({
            "bits": { "enabled": true },
            "power_up": { "enabled": true },
            "new_sub": { "enabled": false },
        });
        let keys = subscription_keys(&alerts);
        assert_eq!(keys.iter().filter(|key| key.as_str() == "bits").count(), 1);
        assert!(!keys.iter().any(|key| key == "subscription"));
    }

    #[test]
    fn templates_are_plain_bounded_data() {
        let alert = twitch::sample_alert_for_test("raid").unwrap();
        let rendered = render_template("{user} brought {viewers}: {message}", &alert, "Raider");
        assert!(rendered.contains("Raider"));
        assert!(rendered.contains("42"));
    }

    #[test]
    fn hype_train_threshold_uses_level_not_contribution_amount() {
        let alert = twitch::sample_alert_for_test("hype_train").unwrap();
        assert_eq!(alert.amount, Some(2_500.0));
        assert_eq!(alert_metric(&alert), 3.0);
        assert_eq!(
            render_template("Level {level}: {progress}/{goal}", &alert, "Someone"),
            "Level 3: 2500/5000"
        );
    }

    #[test]
    fn poll_threshold_and_template_use_leading_choice_votes() {
        let alert = twitch::sample_alert_for_test("poll").unwrap();
        assert_eq!(alert_metric(&alert), 70.0);
        assert_eq!(
            render_template("{winner} won with {votes} votes", &alert, "Someone"),
            "Frost Orb won with 70 votes"
        );
    }

    #[test]
    fn chat_milestone_thresholds_fill_month_and_badge_placeholders() {
        let watch = twitch::sample_alert_for_test("watch_streak").unwrap();
        assert_eq!(alert_metric(&watch), 10.0);
        assert_eq!(
            render_template("{user}: {months}", &watch, "Viewer"),
            "Viewer: 10"
        );

        let modiversary = twitch::sample_alert_for_test("modiversary").unwrap();
        assert_eq!(alert_metric(&modiversary), 24.0);
        assert_eq!(
            render_template("{months} months", &modiversary, "Moderator"),
            "24 months"
        );

        let badge = twitch::sample_alert_for_test("bits_badge").unwrap();
        assert_eq!(alert_metric(&badge), 10_000.0);
        assert_eq!(
            render_template("{threshold} Bits", &badge, "Cheerer"),
            "10000 Bits"
        );
    }

    #[test]
    fn shared_chat_channel_placeholder_uses_twitch_source_field() {
        let alert = twitch::sample_alert_for_test("shared_chat").unwrap();
        assert_eq!(
            render_template("{user} in {channel}: {message}", &alert, "Viewer"),
            "Viewer in PartnerChannel: Hello from the partner channel!"
        );
    }

    #[test]
    fn vault_material_is_not_part_of_settings_or_status() {
        let settings = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!settings.contains("access_token"));
        assert!(!settings.contains("refresh_token"));
        let status = serde_json::to_string(&TwitchStatus::default()).unwrap();
        assert!(!status.contains("token"));
    }
}
