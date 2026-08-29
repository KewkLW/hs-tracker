#![allow(dead_code)]
// The module intentionally exposes a wider crate-local integration surface
// than the first dashboard wiring uses (manual create/delete/list and a file
// token-store fallback included). Keep those supported entry points available
// without turning every unused optional operation into an application warning.

//! Twitch authentication and EventSub support.
//!
//! This module deliberately does not depend on Tauri.  The application owns the
//! storage location and turns [`TwitchServiceEvent`] values into frontend events.
//! OAuth secrets never need to cross that boundary: HS Tracker is a public
//! desktop client and uses Twitch's Device Code flow, which does not use a
//! client secret.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::{Client as HttpClient, Response};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WebSocketError, Message, WebSocket};

const DEVICE_CODE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const REVOKE_URL: &str = "https://id.twitch.tv/oauth2/revoke";
const EVENTSUB_URL: &str = "https://api.twitch.tv/helix/eventsub/subscriptions";
const EVENTSUB_WEBSOCKET_URL: &str = "wss://eventsub.wss.twitch.tv/ws";
const EVENTSUB_RECONNECT_PREFIX: &str = "wss://eventsub.wss.twitch.tv/";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const TOKEN_FILE_VERSION: u32 = 1;
const VALIDATE_EVERY: Duration = Duration::from_secs(60 * 60);
const REFRESH_BEFORE_EXPIRY_SECONDS: i64 = 5 * 60;
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// Errors are intentionally token-free.  Do not add response request bodies or
/// OAuth values to these variants: they may be surfaced in the dashboard log.
#[derive(Debug)]
pub enum TwitchError {
    InvalidConfiguration(String),
    NotAuthorized,
    Api { status: u16, message: String },
    Http(String),
    WebSocket(String),
    Protocol(String),
    Persistence(String),
    Serialization(String),
}

impl fmt::Display for TwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => {
                write!(f, "invalid Twitch configuration: {message}")
            }
            Self::NotAuthorized => f.write_str("Twitch authorization is required"),
            Self::Api { status, message } => write!(f, "Twitch API returned {status}: {message}"),
            Self::Http(message) => write!(f, "Twitch HTTP request failed: {message}"),
            Self::WebSocket(message) => write!(f, "Twitch EventSub connection failed: {message}"),
            Self::Protocol(message) => write!(f, "invalid Twitch response: {message}"),
            Self::Persistence(message) => {
                write!(f, "could not store Twitch authorization: {message}")
            }
            Self::Serialization(message) => write!(f, "could not parse Twitch data: {message}"),
        }
    }
}

impl std::error::Error for TwitchError {}

impl From<reqwest::Error> for TwitchError {
    fn from(value: reqwest::Error) -> Self {
        // reqwest's error display contains the endpoint but not a form body.
        Self::Http(value.without_url().to_string())
    }
}

impl From<serde_json::Error> for TwitchError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

/// Opaque persistence for serialized OAuth tokens.  An application can back
/// this with the OS credential vault without this module knowing the token
/// format.  [`FileTokenStore`] is provided for an app-private data directory.
pub trait TokenPersistence: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, TwitchError>;
    fn save(&self, bytes: &[u8]) -> Result<(), TwitchError>;
    fn clear(&self) -> Result<(), TwitchError>;
}

/// A token file whose exact location is supplied by the caller.  It should be
/// placed in the per-user application-data directory, never next to exported
/// settings.  On Unix the directory and file are restricted to the current
/// user; Windows inherits the per-user application-data ACL.
#[derive(Clone)]
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn ensure_parent(&self) -> Result<(), TwitchError> {
        let parent = self.path.parent().ok_or_else(|| {
            TwitchError::Persistence("the token path has no parent directory".into())
        })?;
        fs::create_dir_all(parent).map_err(persistence_error)?;
        restrict_directory(parent)?;
        Ok(())
    }
}

impl TokenPersistence for FileTokenStore {
    fn load(&self) -> Result<Option<Vec<u8>>, TwitchError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(persistence_error(error)),
        }
    }

    fn save(&self, bytes: &[u8]) -> Result<(), TwitchError> {
        self.ensure_parent()?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        restrict_file_options(&mut options);
        let mut file = options.open(&self.path).map_err(persistence_error)?;
        file.write_all(bytes).map_err(persistence_error)?;
        file.sync_all().map_err(persistence_error)?;
        restrict_file(&self.path)?;
        Ok(())
    }

    fn clear(&self) -> Result<(), TwitchError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(persistence_error(error)),
        }
    }
}

fn persistence_error(error: io::Error) -> TwitchError {
    TwitchError::Persistence(error.to_string())
}

#[cfg(unix)]
fn restrict_file_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn restrict_file_options(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), TwitchError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(persistence_error)
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), TwitchError> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), TwitchError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(persistence_error)
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), TwitchError> {
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct StoredTokenEnvelope {
    version: u32,
    token: OAuthToken,
}

/// Kept private so neither the dashboard command layer nor a debug serializer
/// can accidentally return credentials to JavaScript.
#[derive(Serialize, Deserialize, Clone)]
struct OAuthToken {
    access_token: String,
    refresh_token: String,
    token_type: String,
    scopes: Vec<String>,
    expires_at_unix: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default, alias = "scopes")]
    scope: Vec<String>,
    expires_in: i64,
}

#[derive(Clone, Serialize)]
pub struct DeviceAuthorization {
    /// Required only by the Rust polling command; keep it out of logs.
    #[serde(skip_serializing)]
    device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub issued_at_unix: i64,
}

impl fmt::Debug for DeviceAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceAuthorization")
            .field("device_code", &"[redacted]")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceTokenPoll {
    Pending { retry_after_seconds: u64 },
    SlowDown { retry_after_seconds: u64 },
    Authorized { identity: TwitchIdentity },
    Denied,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TwitchIdentity {
    pub user_id: String,
    pub login: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub expires_in: i64,
}

#[derive(Deserialize)]
struct ValidationResponse {
    client_id: String,
    login: String,
    #[serde(default)]
    scopes: Vec<String>,
    user_id: String,
    expires_in: i64,
}

impl From<ValidationResponse> for TwitchIdentity {
    fn from(value: ValidationResponse) -> Self {
        Self {
            user_id: value.user_id,
            login: value.login,
            client_id: value.client_id,
            scopes: value.scopes,
            expires_in: value.expires_in,
        }
    }
}

#[derive(Clone)]
pub struct TwitchClient {
    client_id: Arc<str>,
    http: HttpClient,
    tokens: Arc<dyn TokenPersistence>,
}

impl TwitchClient {
    pub fn new(
        client_id: impl Into<String>,
        tokens: Arc<dyn TokenPersistence>,
    ) -> Result<Self, TwitchError> {
        let client_id = client_id.into().trim().to_owned();
        if client_id.is_empty()
            || client_id.len() > 128
            || !client_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        {
            return Err(TwitchError::InvalidConfiguration(
                "client ID must contain only ASCII letters and numbers".into(),
            ));
        }

        let http = HttpClient::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent("HS-Tracker/1.0 Twitch-EventSub")
            .build()?;
        Ok(Self {
            client_id: Arc::from(client_id),
            http,
            tokens,
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Starts Twitch's public-client Device Code flow.  The returned user code
    /// and URI are safe to show; the device code remains Rust-only.
    pub fn begin_device_authorization(
        &self,
        scopes: &[String],
    ) -> Result<DeviceAuthorization, TwitchError> {
        let scopes = normalize_scopes(scopes.iter().map(String::as_str));
        let response = self
            .http
            .post(DEVICE_CODE_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("client_id", self.client_id()),
                ("scopes", &scopes.join(" ")),
            ])
            .send()?;
        let body: DeviceAuthorizationResponse = parse_json_response(response)?;
        Ok(DeviceAuthorization {
            device_code: body.device_code,
            user_code: body.user_code,
            verification_uri: body.verification_uri,
            verification_uri_complete: body.verification_uri_complete,
            expires_in: body.expires_in,
            interval: body.interval.max(1),
            issued_at_unix: unix_now(),
        })
    }

    /// Polls exactly once.  The UI should wait the returned interval before
    /// calling again, keeping authorization cancellation responsive.
    pub fn poll_device_authorization(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<DeviceTokenPoll, TwitchError> {
        if unix_now() >= authorization.issued_at_unix + authorization.expires_in as i64 {
            return Ok(DeviceTokenPoll::Expired);
        }

        let response = self
            .http
            .post(TOKEN_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("client_id", self.client_id()),
                ("device_code", authorization.device_code.as_str()),
                ("grant_type", DEVICE_GRANT),
            ])
            .send()?;

        if response.status().is_success() {
            let raw: TokenResponse = response.json()?;
            self.store_token(raw)?;
            let identity = self.validate()?;
            return Ok(DeviceTokenPoll::Authorized { identity });
        }

        let (status, message) = response_error(response);
        let normalized = message.to_ascii_lowercase().replace([' ', '-'], "_");
        if normalized.contains("authorization_pending") {
            Ok(DeviceTokenPoll::Pending {
                retry_after_seconds: authorization.interval,
            })
        } else if normalized.contains("slow_down") {
            Ok(DeviceTokenPoll::SlowDown {
                retry_after_seconds: authorization.interval.saturating_add(5),
            })
        } else if normalized.contains("access_denied") || normalized.contains("denied") {
            Ok(DeviceTokenPoll::Denied)
        } else if normalized.contains("expired") {
            Ok(DeviceTokenPoll::Expired)
        } else {
            Err(TwitchError::Api { status, message })
        }
    }

    /// Validates the current access token.  Twitch requires third-party apps to
    /// do this at startup and once per hour while active.
    pub fn validate(&self) -> Result<TwitchIdentity, TwitchError> {
        let token = self.load_token()?.ok_or(TwitchError::NotAuthorized)?;
        self.validate_access_token(&token.access_token)
    }

    /// Returns a validated token and refreshes it when it is near expiry or no
    /// longer valid.  Refresh-token rotation is persisted before returning.
    pub fn ensure_authorized(&self) -> Result<TwitchIdentity, TwitchError> {
        let mut token = self.load_token()?.ok_or(TwitchError::NotAuthorized)?;
        let close_to_expiry = token.expires_at_unix - unix_now() <= REFRESH_BEFORE_EXPIRY_SECONDS;

        if !close_to_expiry {
            match self.validate_access_token(&token.access_token) {
                Ok(identity) if identity.expires_in > REFRESH_BEFORE_EXPIRY_SECONDS => {
                    self.ensure_matching_client(&identity)?;
                    return Ok(identity);
                }
                Ok(_) | Err(TwitchError::Api { status: 401, .. }) => {}
                Err(error) => return Err(error),
            }
        }

        token = self.refresh_token(&token.refresh_token)?;
        let identity = self.validate_access_token(&token.access_token)?;
        self.ensure_matching_client(&identity)?;
        Ok(identity)
    }

    pub fn has_authorization(&self) -> Result<bool, TwitchError> {
        Ok(self.load_token()?.is_some())
    }

    /// Revokes the current access token when possible and always removes the
    /// local copy.  This is the command to use for a Disconnect button.
    pub fn disconnect(&self) -> Result<(), TwitchError> {
        let token = self.load_token()?;
        if let Some(token) = token {
            let response = self
                .http
                .post(REVOKE_URL)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .form(&[
                    ("client_id", self.client_id()),
                    ("token", token.access_token.as_str()),
                ])
                .send();
            // A locally expired/revoked token commonly returns 400.  Disconnect
            // must still complete locally; only transport errors are ignored.
            let _ = response;
        }
        self.tokens.clear()
    }

    pub fn create_subscription(
        &self,
        session_id: &str,
        request: &EventSubscriptionRequest,
    ) -> Result<CreatedSubscription, TwitchError> {
        let (token, identity) = self.authorized_token()?;
        self.create_subscription_with_token(&token, &identity, session_id, request)
    }

    pub fn delete_subscription(&self, subscription_id: &str) -> Result<(), TwitchError> {
        if subscription_id.is_empty() {
            return Err(TwitchError::InvalidConfiguration(
                "subscription ID is empty".into(),
            ));
        }
        let (token, _) = self.authorized_token()?;
        let url = eventsub_url_with_param("id", subscription_id)?;
        let response = self
            .http
            .delete(url)
            .header("Client-Id", self.client_id())
            .header(AUTHORIZATION, format!("Bearer {}", token.access_token))
            .send()?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(api_error(response))
        }
    }

    pub fn list_subscriptions(&self) -> Result<Vec<CreatedSubscription>, TwitchError> {
        let (token, _) = self.authorized_token()?;
        let mut after: Option<String> = None;
        let mut all = Vec::new();
        loop {
            let url = if let Some(cursor) = &after {
                eventsub_url_with_param("after", cursor)?
            } else {
                reqwest::Url::parse(EVENTSUB_URL)
                    .map_err(|error| TwitchError::InvalidConfiguration(error.to_string()))?
            };
            let request = self
                .http
                .get(url)
                .header("Client-Id", self.client_id())
                .header(AUTHORIZATION, format!("Bearer {}", token.access_token));
            let page: SubscriptionListResponse = parse_json_response(request.send()?)?;
            all.extend(page.data);
            after = page.pagination.cursor;
            if after.is_none() {
                break;
            }
        }
        Ok(all)
    }

    fn authorized_token(&self) -> Result<(OAuthToken, TwitchIdentity), TwitchError> {
        let identity = self.ensure_authorized()?;
        let token = self.load_token()?.ok_or(TwitchError::NotAuthorized)?;
        Ok((token, identity))
    }

    fn ensure_matching_client(&self, identity: &TwitchIdentity) -> Result<(), TwitchError> {
        if identity.client_id != self.client_id() {
            return Err(TwitchError::InvalidConfiguration(
                "the stored Twitch token belongs to another client ID".into(),
            ));
        }
        Ok(())
    }

    fn validate_access_token(&self, access_token: &str) -> Result<TwitchIdentity, TwitchError> {
        let response = self
            .http
            .get(VALIDATE_URL)
            .header(AUTHORIZATION, format!("OAuth {access_token}"))
            .send()?;
        let validation: ValidationResponse = parse_json_response(response)?;
        let identity = TwitchIdentity::from(validation);
        self.ensure_matching_client(&identity)?;
        Ok(identity)
    }

    fn refresh_token(&self, refresh_token: &str) -> Result<OAuthToken, TwitchError> {
        let response = self
            .http
            .post(TOKEN_URL)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", self.client_id()),
            ])
            .send()?;
        if !response.status().is_success() {
            if matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
            ) {
                let _ = self.tokens.clear();
                return Err(TwitchError::NotAuthorized);
            }
            return Err(api_error(response));
        }
        let raw: TokenResponse = response.json()?;
        self.store_token(raw)
    }

    fn store_token(&self, raw: TokenResponse) -> Result<OAuthToken, TwitchError> {
        if raw.access_token.is_empty() || raw.refresh_token.is_empty() {
            return Err(TwitchError::Protocol(
                "the OAuth response omitted a required token".into(),
            ));
        }
        let token = OAuthToken {
            access_token: raw.access_token,
            refresh_token: raw.refresh_token,
            token_type: raw.token_type,
            scopes: normalize_scopes(raw.scope.iter().map(String::as_str)),
            expires_at_unix: unix_now().saturating_add(raw.expires_in.max(0)),
        };
        let envelope = StoredTokenEnvelope {
            version: TOKEN_FILE_VERSION,
            token: token.clone(),
        };
        self.tokens.save(&serde_json::to_vec(&envelope)?)?;
        Ok(token)
    }

    fn load_token(&self) -> Result<Option<OAuthToken>, TwitchError> {
        let Some(bytes) = self.tokens.load()? else {
            return Ok(None);
        };
        let envelope: StoredTokenEnvelope = serde_json::from_slice(&bytes).map_err(|error| {
            TwitchError::Persistence(format!("stored token data is invalid: {error}"))
        })?;
        if envelope.version != TOKEN_FILE_VERSION {
            return Err(TwitchError::Persistence(format!(
                "unsupported stored token version {}",
                envelope.version
            )));
        }
        Ok(Some(envelope.token))
    }

    fn create_subscription_with_token(
        &self,
        token: &OAuthToken,
        identity: &TwitchIdentity,
        session_id: &str,
        request: &EventSubscriptionRequest,
    ) -> Result<CreatedSubscription, TwitchError> {
        if session_id.is_empty() || request.event_type.is_empty() || request.version.is_empty() {
            return Err(TwitchError::InvalidConfiguration(
                "EventSub session, type, and version are required".into(),
            ));
        }
        self.ensure_matching_client(identity)?;
        let body = json!({
            "type": request.event_type,
            "version": request.version,
            "condition": request.condition,
            "transport": {
                "method": "websocket",
                "session_id": session_id,
            }
        });
        let response = self
            .http
            .post(EVENTSUB_URL)
            .header("Client-Id", self.client_id())
            .header(AUTHORIZATION, format!("Bearer {}", token.access_token))
            .json(&body)
            .send()?;
        let result: SubscriptionListResponse = parse_json_response(response)?;
        result
            .data
            .into_iter()
            .next()
            .ok_or_else(|| TwitchError::Protocol("EventSub create returned no subscription".into()))
    }
}

fn unix_now() -> i64 {
    current_unix_millis() / 1_000
}

/// JavaScript `Date`-compatible timestamp for status payloads.
pub fn current_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn normalize_scopes<'a>(scopes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    scopes
        .into_iter()
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn response_error(response: Response) -> (u16, String) {
    let status = response.status().as_u16();
    let value = response.json::<Value>().unwrap_or(Value::Null);
    let message = value
        .get("message")
        .or_else(|| value.get("error"))
        .or_else(|| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("request failed")
        .chars()
        .take(500)
        .collect();
    (status, message)
}

fn api_error(response: Response) -> TwitchError {
    let (status, message) = response_error(response);
    TwitchError::Api { status, message }
}

fn parse_json_response<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T, TwitchError> {
    if response.status().is_success() {
        Ok(response.json()?)
    } else {
        Err(api_error(response))
    }
}

fn eventsub_url_with_param(key: &str, value: &str) -> Result<reqwest::Url, TwitchError> {
    let mut url = reqwest::Url::parse(EVENTSUB_URL)
        .map_err(|error| TwitchError::InvalidConfiguration(error.to_string()))?;
    url.query_pairs_mut().append_pair(key, value);
    Ok(url)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventSubscriptionRequest {
    #[serde(rename = "type")]
    pub event_type: String,
    pub version: String,
    pub condition: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreatedSubscription {
    pub id: String,
    pub status: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub version: String,
    #[serde(default)]
    pub condition: BTreeMap<String, String>,
    #[serde(default)]
    pub cost: u32,
}

#[derive(Deserialize)]
struct SubscriptionListResponse {
    #[serde(default)]
    data: Vec<CreatedSubscription>,
    #[serde(default)]
    pagination: Pagination,
}

#[derive(Default, Deserialize)]
struct Pagination {
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TwitchAlertKind {
    Follow,
    Subscription,
    Resubscription,
    GiftSubscription,
    Cheer,
    PowerUp,
    Raid,
    OutgoingRaid,
    RewardRedemption,
    AutomaticReward,
    CharityDonation,
    HypeTrainBegin,
    HypeTrainProgress,
    HypeTrainEnd,
    GoalBegin,
    GoalProgress,
    GoalEnd,
    PollBegin,
    PollProgress,
    PollEnd,
    PredictionBegin,
    PredictionProgress,
    PredictionLock,
    PredictionEnd,
    CharityCampaignStart,
    CharityCampaignProgress,
    CharityCampaignStop,
    ShoutoutCreated,
    ShoutoutReceived,
    StreamOnline,
    StreamOffline,
    AdBreak,
    ChannelUpdate,
    ChatUpgrade,
    PayItForward,
    WatchStreak,
    Modiversary,
    BitsBadge,
    Announcement,
    UserIntro,
    SharedChat,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TwitchAlert {
    pub id: String,
    pub kind: TwitchAlertKind,
    pub source_type: String,
    pub timestamp: String,
    pub title: String,
    pub user_id: Option<String>,
    pub user_login: Option<String>,
    pub user_name: Option<String>,
    pub secondary_user_name: Option<String>,
    pub message: Option<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub count: Option<u64>,
    pub tier: Option<String>,
    pub anonymous: bool,
    #[serde(default)]
    pub details: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TwitchAlertCatalogEntry {
    pub key: String,
    pub label: String,
    pub category: String,
    pub description: String,
    pub event_type: String,
    pub version: String,
    pub required_scope: Option<String>,
    pub default_enabled: bool,
    pub alert_kinds: Vec<TwitchAlertKind>,
}

#[derive(Clone, Copy)]
enum ConditionKind {
    Broadcaster,
    BroadcasterAndModerator,
    BroadcasterAndUser,
    ToBroadcaster,
    FromBroadcaster,
}

#[derive(Clone, Copy)]
struct CatalogSpec {
    key: &'static str,
    label: &'static str,
    category: &'static str,
    description: &'static str,
    event_type: &'static str,
    version: &'static str,
    required_scope: Option<&'static str>,
    default_enabled: bool,
    condition: ConditionKind,
    alert_kinds: &'static [TwitchAlertKind],
}

const CATALOG: &[CatalogSpec] = &[
    CatalogSpec { key: "follow", label: "Followers", category: "Community", description: "A viewer follows the channel.", event_type: "channel.follow", version: "2", required_scope: Some("moderator:read:followers"), default_enabled: true, condition: ConditionKind::BroadcasterAndModerator, alert_kinds: &[TwitchAlertKind::Follow] },
    CatalogSpec { key: "subscription", label: "New subscriptions", category: "Subscriptions", description: "A viewer subscribes. Gift recipients are ignored here to prevent duplicate gift alerts.", event_type: "channel.subscribe", version: "1", required_scope: Some("channel:read:subscriptions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::Subscription] },
    CatalogSpec { key: "resubscription", label: "Resubscriptions", category: "Subscriptions", description: "A subscriber shares a resubscription message.", event_type: "channel.subscription.message", version: "1", required_scope: Some("channel:read:subscriptions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::Resubscription] },
    CatalogSpec { key: "gift_subscription", label: "Gift subscriptions", category: "Subscriptions", description: "A viewer gifts one or more subscriptions.", event_type: "channel.subscription.gift", version: "1", required_scope: Some("channel:read:subscriptions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::GiftSubscription] },
    CatalogSpec { key: "bits", label: "Bits, cheers & power-ups", category: "Support", description: "A viewer uses Bits for a cheer or power-up. This replaces overlapping legacy Bits events.", event_type: "channel.bits.use", version: "1", required_scope: Some("bits:read"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::Cheer, TwitchAlertKind::PowerUp] },
    CatalogSpec { key: "raid", label: "Incoming raids", category: "Community", description: "Another broadcaster raids this channel.", event_type: "channel.raid", version: "1", required_scope: None, default_enabled: true, condition: ConditionKind::ToBroadcaster, alert_kinds: &[TwitchAlertKind::Raid] },
    CatalogSpec { key: "outgoing_raid", label: "Outgoing raids", category: "Optional", description: "This channel raids another broadcaster.", event_type: "channel.raid", version: "1", required_scope: None, default_enabled: false, condition: ConditionKind::FromBroadcaster, alert_kinds: &[TwitchAlertKind::OutgoingRaid] },
    CatalogSpec { key: "custom_reward", label: "Custom channel-point rewards", category: "Channel Points", description: "A viewer redeems a custom channel-point reward.", event_type: "channel.channel_points_custom_reward_redemption.add", version: "1", required_scope: Some("channel:read:redemptions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::RewardRedemption] },
    CatalogSpec { key: "automatic_reward", label: "Automatic channel-point rewards", category: "Channel Points", description: "A viewer redeems an automatic Twitch reward.", event_type: "channel.channel_points_automatic_reward_redemption.add", version: "2", required_scope: Some("channel:read:redemptions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::AutomaticReward] },
    CatalogSpec { key: "charity_donation", label: "Charity donations", category: "Charity", description: "A viewer donates to the active charity campaign.", event_type: "channel.charity_campaign.donate", version: "1", required_scope: Some("channel:read:charity"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::CharityDonation] },
    CatalogSpec { key: "chat_milestones", label: "Chat milestones", category: "Community", description: "Paid upgrades, pay-it-forward, milestones, announcements, introductions, and Bits badges. Overlapping subscription, raid, and charity notices are ignored.", event_type: "channel.chat.notification", version: "1", required_scope: Some("user:read:chat"), default_enabled: true, condition: ConditionKind::BroadcasterAndUser, alert_kinds: &[TwitchAlertKind::ChatUpgrade, TwitchAlertKind::PayItForward, TwitchAlertKind::WatchStreak, TwitchAlertKind::Modiversary, TwitchAlertKind::BitsBadge, TwitchAlertKind::Announcement, TwitchAlertKind::UserIntro] },
    CatalogSpec { key: "shared_chat", label: "Shared Chat activity", category: "Optional", description: "Messages arriving from another channel through Shared Chat. Ordinary same-channel messages are suppressed.", event_type: "channel.chat.message", version: "1", required_scope: Some("user:read:chat"), default_enabled: false, condition: ConditionKind::BroadcasterAndUser, alert_kinds: &[TwitchAlertKind::SharedChat] },
    CatalogSpec { key: "hype_train_begin", label: "Hype Train starts", category: "Hype Train", description: "A Hype Train starts.", event_type: "channel.hype_train.begin", version: "2", required_scope: Some("channel:read:hype_train"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::HypeTrainBegin] },
    CatalogSpec { key: "hype_train_progress", label: "Hype Train progress", category: "Hype Train", description: "A Hype Train advances. This can be frequent.", event_type: "channel.hype_train.progress", version: "2", required_scope: Some("channel:read:hype_train"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::HypeTrainProgress] },
    CatalogSpec { key: "hype_train_end", label: "Hype Train ends", category: "Hype Train", description: "A Hype Train finishes.", event_type: "channel.hype_train.end", version: "2", required_scope: Some("channel:read:hype_train"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::HypeTrainEnd] },
    CatalogSpec { key: "goal_begin", label: "Creator goal starts", category: "Goals", description: "A follower, subscription, or other creator goal begins.", event_type: "channel.goal.begin", version: "1", required_scope: Some("channel:read:goals"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::GoalBegin] },
    CatalogSpec { key: "goal_progress", label: "Creator goal progress", category: "Goals", description: "A creator goal advances. This can be frequent.", event_type: "channel.goal.progress", version: "1", required_scope: Some("channel:read:goals"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::GoalProgress] },
    CatalogSpec { key: "goal_end", label: "Creator goal ends", category: "Goals", description: "A creator goal completes or ends.", event_type: "channel.goal.end", version: "1", required_scope: Some("channel:read:goals"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::GoalEnd] },
    CatalogSpec { key: "poll_begin", label: "Poll starts", category: "Engagement", description: "A channel poll starts.", event_type: "channel.poll.begin", version: "1", required_scope: Some("channel:read:polls"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PollBegin] },
    CatalogSpec { key: "poll_progress", label: "Poll progress", category: "Engagement", description: "A channel poll changes. This can be frequent.", event_type: "channel.poll.progress", version: "1", required_scope: Some("channel:read:polls"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PollProgress] },
    CatalogSpec { key: "poll_end", label: "Poll ends", category: "Engagement", description: "A channel poll ends.", event_type: "channel.poll.end", version: "1", required_scope: Some("channel:read:polls"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PollEnd] },
    CatalogSpec { key: "prediction_begin", label: "Prediction starts", category: "Engagement", description: "A channel prediction starts.", event_type: "channel.prediction.begin", version: "1", required_scope: Some("channel:read:predictions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PredictionBegin] },
    CatalogSpec { key: "prediction_progress", label: "Prediction progress", category: "Engagement", description: "A prediction changes. This can be frequent.", event_type: "channel.prediction.progress", version: "1", required_scope: Some("channel:read:predictions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PredictionProgress] },
    CatalogSpec { key: "prediction_lock", label: "Prediction locks", category: "Engagement", description: "A channel prediction locks.", event_type: "channel.prediction.lock", version: "1", required_scope: Some("channel:read:predictions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PredictionLock] },
    CatalogSpec { key: "prediction_end", label: "Prediction ends", category: "Engagement", description: "A channel prediction resolves or is canceled.", event_type: "channel.prediction.end", version: "1", required_scope: Some("channel:read:predictions"), default_enabled: true, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::PredictionEnd] },
    CatalogSpec { key: "charity_start", label: "Charity campaign starts", category: "Charity", description: "A charity campaign starts.", event_type: "channel.charity_campaign.start", version: "1", required_scope: Some("channel:read:charity"), default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::CharityCampaignStart] },
    CatalogSpec { key: "charity_progress", label: "Charity campaign progress", category: "Charity", description: "The charity total changes. Donation alerts normally provide enough detail.", event_type: "channel.charity_campaign.progress", version: "1", required_scope: Some("channel:read:charity"), default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::CharityCampaignProgress] },
    CatalogSpec { key: "charity_stop", label: "Charity campaign ends", category: "Charity", description: "A charity campaign stops.", event_type: "channel.charity_campaign.stop", version: "1", required_scope: Some("channel:read:charity"), default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::CharityCampaignStop] },
    CatalogSpec { key: "shoutout_created", label: "Shoutouts sent", category: "Community", description: "A moderator sends a shoutout from this channel.", event_type: "channel.shoutout.create", version: "1", required_scope: Some("moderator:read:shoutouts"), default_enabled: true, condition: ConditionKind::BroadcasterAndModerator, alert_kinds: &[TwitchAlertKind::ShoutoutCreated] },
    CatalogSpec { key: "shoutout_received", label: "Shoutouts received", category: "Community", description: "Another broadcaster shouts out this channel.", event_type: "channel.shoutout.receive", version: "1", required_scope: Some("moderator:read:shoutouts"), default_enabled: true, condition: ConditionKind::BroadcasterAndModerator, alert_kinds: &[TwitchAlertKind::ShoutoutReceived] },
    CatalogSpec { key: "stream_online", label: "Stream online", category: "Stream", description: "This channel goes live.", event_type: "stream.online", version: "1", required_scope: None, default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::StreamOnline] },
    CatalogSpec { key: "stream_offline", label: "Stream offline", category: "Stream", description: "This channel goes offline.", event_type: "stream.offline", version: "1", required_scope: None, default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::StreamOffline] },
    CatalogSpec { key: "ad_break", label: "Ad breaks", category: "Stream", description: "An ad break starts.", event_type: "channel.ad_break.begin", version: "1", required_scope: Some("channel:read:ads"), default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::AdBreak] },
    CatalogSpec { key: "channel_update", label: "Channel updates", category: "Stream", description: "The stream title, category, language, or content labels change.", event_type: "channel.update", version: "2", required_scope: None, default_enabled: false, condition: ConditionKind::Broadcaster, alert_kinds: &[TwitchAlertKind::ChannelUpdate] },
];

pub fn twitch_alert_catalog() -> Vec<TwitchAlertCatalogEntry> {
    CATALOG
        .iter()
        .map(|entry| TwitchAlertCatalogEntry {
            key: entry.key.into(),
            label: entry.label.into(),
            category: entry.category.into(),
            description: entry.description.into(),
            event_type: entry.event_type.into(),
            version: entry.version.into(),
            required_scope: entry.required_scope.map(str::to_owned),
            default_enabled: entry.default_enabled,
            alert_kinds: entry.alert_kinds.to_vec(),
        })
        .collect()
}

pub fn default_subscription_keys() -> Vec<String> {
    CATALOG
        .iter()
        .filter(|entry| entry.default_enabled)
        .map(|entry| entry.key.to_owned())
        .collect()
}

/// Converts the dashboard's aggregate alert keys into the exact EventSub
/// catalog keys. Shared transports such as Bits and Chat are returned once.
pub fn subscription_keys_for_logical_alerts<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut subscriptions = BTreeSet::new();
    for key in keys {
        let mapped: &[&str] = match key {
            "follow" => &["follow"],
            "new_sub" => &["subscription"],
            "resub" => &["resubscription"],
            "sub_gift" => &["gift_subscription"],
            "bits" | "power_up" => &["bits"],
            "raid" => &["raid"],
            "outgoing_raid" => &["outgoing_raid"],
            "channel_points" => &["custom_reward"],
            "automatic_points" => &["automatic_reward"],
            "charity_donation" => &["charity_donation"],
            "hype_train" => &["hype_train_begin", "hype_train_progress", "hype_train_end"],
            "goal" => &["goal_begin", "goal_progress", "goal_end"],
            "poll" => &["poll_begin", "poll_progress", "poll_end"],
            "prediction" => &[
                "prediction_begin",
                "prediction_progress",
                "prediction_lock",
                "prediction_end",
            ],
            "charity_campaign" => &["charity_start", "charity_progress", "charity_stop"],
            "shoutout" => &["shoutout_created", "shoutout_received"],
            "stream_online" => &["stream_online"],
            "stream_offline" => &["stream_offline"],
            "ad_break" => &["ad_break"],
            "channel_update" => &["channel_update"],
            "chat_announcement" | "watch_streak" | "modiversary" | "bits_badge" | "user_intro"
            | "sub_upgrade" | "pay_it_forward" => &["chat_milestones"],
            "shared_chat" => &["shared_chat"],
            _ => &[],
        };
        subscriptions.extend(mapped.iter().map(|key| (*key).to_owned()));
    }
    subscriptions.into_iter().collect()
}

pub fn required_scopes_for_logical_alerts<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let subscriptions = subscription_keys_for_logical_alerts(keys);
    required_scopes(subscriptions.iter().map(String::as_str))
}

pub fn required_scopes<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let selected: HashSet<&str> = keys.into_iter().collect();
    CATALOG
        .iter()
        .filter(|entry| selected.contains(entry.key))
        .filter_map(|entry| entry.required_scope)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn all_supported_scopes() -> Vec<String> {
    required_scopes(CATALOG.iter().map(|entry| entry.key))
}

pub fn event_subscription_request(
    key: &str,
    broadcaster_user_id: &str,
    authenticated_user_id: &str,
) -> Result<EventSubscriptionRequest, TwitchError> {
    let entry = CATALOG
        .iter()
        .find(|entry| entry.key == key)
        .ok_or_else(|| TwitchError::InvalidConfiguration(format!("unknown alert key {key:?}")))?;
    if !valid_twitch_id(broadcaster_user_id) || !valid_twitch_id(authenticated_user_id) {
        return Err(TwitchError::InvalidConfiguration(
            "Twitch user IDs must contain only decimal digits".into(),
        ));
    }

    let mut condition = BTreeMap::new();
    match entry.condition {
        ConditionKind::Broadcaster => {
            condition.insert("broadcaster_user_id".into(), broadcaster_user_id.into());
        }
        ConditionKind::BroadcasterAndModerator => {
            condition.insert("broadcaster_user_id".into(), broadcaster_user_id.into());
            condition.insert("moderator_user_id".into(), authenticated_user_id.into());
        }
        ConditionKind::BroadcasterAndUser => {
            condition.insert("broadcaster_user_id".into(), broadcaster_user_id.into());
            condition.insert("user_id".into(), authenticated_user_id.into());
        }
        ConditionKind::ToBroadcaster => {
            condition.insert("to_broadcaster_user_id".into(), broadcaster_user_id.into());
        }
        ConditionKind::FromBroadcaster => {
            condition.insert(
                "from_broadcaster_user_id".into(),
                broadcaster_user_id.into(),
            );
        }
    }
    Ok(EventSubscriptionRequest {
        event_type: entry.event_type.into(),
        version: entry.version.into(),
        condition,
    })
}

fn valid_twitch_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.chars().all(|character| character.is_ascii_digit())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TwitchEventSubConfig {
    pub broadcaster_user_id: String,
    pub enabled_subscriptions: Vec<String>,
}

impl Default for TwitchEventSubConfig {
    fn default() -> Self {
        Self {
            broadcaster_user_id: String::new(),
            enabled_subscriptions: default_subscription_keys(),
        }
    }
}

impl TwitchEventSubConfig {
    pub fn for_broadcaster(broadcaster_user_id: impl Into<String>) -> Self {
        Self {
            broadcaster_user_id: broadcaster_user_id.into(),
            ..Self::default()
        }
    }

    pub fn for_logical_alerts<'a>(
        broadcaster_user_id: impl Into<String>,
        alert_keys: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Self {
            broadcaster_user_id: broadcaster_user_id.into(),
            enabled_subscriptions: subscription_keys_for_logical_alerts(alert_keys),
        }
    }

    fn normalized_keys(&self) -> Vec<&'static CatalogSpec> {
        let selected: HashSet<&str> = self
            .enabled_subscriptions
            .iter()
            .map(String::as_str)
            .collect();
        CATALOG
            .iter()
            .filter(|entry| selected.contains(entry.key))
            .collect()
    }
}

#[derive(Deserialize)]
struct EventSubEnvelope {
    metadata: EventSubMetadata,
    #[serde(default)]
    payload: Value,
}

#[derive(Deserialize)]
struct EventSubMetadata {
    #[serde(default)]
    message_id: String,
    #[serde(default)]
    message_type: String,
    #[serde(default)]
    message_timestamp: String,
    #[serde(default)]
    subscription_type: String,
    #[serde(default, rename = "subscription_version")]
    _subscription_version: String,
}

/// Maps a raw EventSub WebSocket JSON message into one alert.  Session control
/// messages and deliberately suppressed duplicates return `None`.
pub fn normalize_eventsub_message(text: &str) -> Result<Option<TwitchAlert>, TwitchError> {
    let envelope: EventSubEnvelope = serde_json::from_str(text)?;
    if envelope.metadata.message_type != "notification" {
        return Ok(None);
    }
    Ok(normalize_notification(&envelope))
}

fn normalize_notification(envelope: &EventSubEnvelope) -> Option<TwitchAlert> {
    let event_type = if envelope.metadata.subscription_type.is_empty() {
        envelope
            .payload
            .pointer("/subscription/type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        envelope.metadata.subscription_type.as_str()
    };
    let event = envelope.payload.get("event")?;
    let dispatch_type = if event_type == "channel.raid"
        && envelope
            .payload
            .pointer("/subscription/condition/from_broadcaster_user_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
    {
        "channel.raid.outgoing"
    } else {
        event_type
    };
    let (kind, title) = alert_kind_and_title(dispatch_type, event)?;

    let mut details = BTreeMap::new();
    collect_common_details(dispatch_type, event, &mut details);
    let (amount, currency) = extract_amount(dispatch_type, event);
    let count = extract_count(dispatch_type, event);
    let message = extract_message(dispatch_type, event);
    let tier = string_at_any(event, &["/tier", "/sub/tier"]);

    Some(TwitchAlert {
        id: envelope.metadata.message_id.clone(),
        kind,
        source_type: event_type.to_owned(),
        timestamp: envelope.metadata.message_timestamp.clone(),
        title: title.into(),
        user_id: string_at_any(event, user_paths(dispatch_type, "id")),
        user_login: string_at_any(event, user_paths(dispatch_type, "login")),
        user_name: string_at_any(event, user_paths(dispatch_type, "name")),
        secondary_user_name: secondary_user_name(dispatch_type, event),
        message,
        amount,
        currency,
        count,
        tier,
        anonymous: event
            .get("is_anonymous")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        details,
    })
}

fn alert_kind_and_title(
    event_type: &str,
    event: &Value,
) -> Option<(TwitchAlertKind, &'static str)> {
    Some(match event_type {
        "channel.follow" => (TwitchAlertKind::Follow, "New follower"),
        "channel.subscribe" => {
            if event
                .get("is_gift")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            (TwitchAlertKind::Subscription, "New subscriber")
        }
        "channel.subscription.message" => (TwitchAlertKind::Resubscription, "Resubscription"),
        "channel.subscription.gift" => (TwitchAlertKind::GiftSubscription, "Gift subscriptions"),
        "channel.bits.use" | "channel.cheer" => {
            let usage = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if usage.contains("power_up") || event.get("power_up").is_some_and(|v| !v.is_null()) {
                (TwitchAlertKind::PowerUp, "Power-up")
            } else {
                (TwitchAlertKind::Cheer, "Cheer")
            }
        }
        "channel.raid" => (TwitchAlertKind::Raid, "Incoming raid"),
        "channel.raid.outgoing" => (TwitchAlertKind::OutgoingRaid, "Outgoing raid"),
        "channel.channel_points_custom_reward_redemption.add" => {
            (TwitchAlertKind::RewardRedemption, "Channel-point reward")
        }
        "channel.channel_points_automatic_reward_redemption.add" => {
            (TwitchAlertKind::AutomaticReward, "Automatic reward")
        }
        "channel.charity_campaign.donate" => (TwitchAlertKind::CharityDonation, "Charity donation"),
        "channel.hype_train.begin" => (TwitchAlertKind::HypeTrainBegin, "Hype Train started"),
        "channel.hype_train.progress" => {
            (TwitchAlertKind::HypeTrainProgress, "Hype Train progress")
        }
        "channel.hype_train.end" => (TwitchAlertKind::HypeTrainEnd, "Hype Train ended"),
        "channel.goal.begin" => (TwitchAlertKind::GoalBegin, "Creator goal started"),
        "channel.goal.progress" => (TwitchAlertKind::GoalProgress, "Creator goal progress"),
        "channel.goal.end" => (TwitchAlertKind::GoalEnd, "Creator goal ended"),
        "channel.poll.begin" => (TwitchAlertKind::PollBegin, "Poll started"),
        "channel.poll.progress" => (TwitchAlertKind::PollProgress, "Poll progress"),
        "channel.poll.end" => (TwitchAlertKind::PollEnd, "Poll ended"),
        "channel.prediction.begin" => (TwitchAlertKind::PredictionBegin, "Prediction started"),
        "channel.prediction.progress" => {
            (TwitchAlertKind::PredictionProgress, "Prediction progress")
        }
        "channel.prediction.lock" => (TwitchAlertKind::PredictionLock, "Prediction locked"),
        "channel.prediction.end" => (TwitchAlertKind::PredictionEnd, "Prediction ended"),
        "channel.charity_campaign.start" => (
            TwitchAlertKind::CharityCampaignStart,
            "Charity campaign started",
        ),
        "channel.charity_campaign.progress" => (
            TwitchAlertKind::CharityCampaignProgress,
            "Charity campaign progress",
        ),
        "channel.charity_campaign.stop" => (
            TwitchAlertKind::CharityCampaignStop,
            "Charity campaign ended",
        ),
        "channel.shoutout.create" => (TwitchAlertKind::ShoutoutCreated, "Shoutout sent"),
        "channel.shoutout.receive" => (TwitchAlertKind::ShoutoutReceived, "Shoutout received"),
        "stream.online" => (TwitchAlertKind::StreamOnline, "Stream online"),
        "stream.offline" => (TwitchAlertKind::StreamOffline, "Stream offline"),
        "channel.ad_break.begin" => (TwitchAlertKind::AdBreak, "Ad break started"),
        "channel.update" => (TwitchAlertKind::ChannelUpdate, "Channel updated"),
        "channel.chat.notification" => chat_notification_kind(event)?,
        "channel.chat.message" => {
            let source = event
                .get("source_broadcaster_user_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let broadcaster = event
                .get("broadcaster_user_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if source == broadcaster {
                return None;
            }
            (TwitchAlertKind::SharedChat, "Shared Chat")
        }
        _ => return None,
    })
}

fn chat_notification_kind(event: &Value) -> Option<(TwitchAlertKind, &'static str)> {
    let notice = event.get("notice_type").and_then(Value::as_str)?;
    Some(match notice {
        // Dedicated EventSub subscriptions already generate these alerts.
        "sub"
        | "resub"
        | "sub_gift"
        | "community_sub_gift"
        | "raid"
        | "unraid"
        | "charity_donation"
        | "shared_chat_sub"
        | "shared_chat_resub"
        | "shared_chat_sub_gift"
        | "shared_chat_community_sub_gift"
        | "shared_chat_raid" => return None,
        "gift_paid_upgrade"
        | "prime_paid_upgrade"
        | "shared_chat_prime_paid_upgrade"
        | "shared_chat_gift_paid_upgrade" => {
            (TwitchAlertKind::ChatUpgrade, "Subscription upgraded")
        }
        "pay_it_forward" | "shared_chat_pay_it_forward" => {
            (TwitchAlertKind::PayItForward, "Pay it forward")
        }
        "bits_badge_tier" | "shared_chat_bits_badge_tier" => {
            (TwitchAlertKind::BitsBadge, "Bits badge unlocked")
        }
        "announcement" | "shared_chat_announcement" => {
            (TwitchAlertKind::Announcement, "Announcement")
        }
        "user_intro" | "shared_chat_user_intro" => {
            (TwitchAlertKind::UserIntro, "First-time chatter")
        }
        "watch_streak" | "view_milestone" | "shared_chat_watch_streak" => {
            (TwitchAlertKind::WatchStreak, "Watch streak")
        }
        "modiversary" | "shared_chat_modiversary" => {
            (TwitchAlertKind::Modiversary, "Mod anniversary")
        }
        _ => return None,
    })
}

fn user_paths(event_type: &str, field: &str) -> &'static [&'static str] {
    match (event_type, field) {
        ("channel.raid", "id") => &["/from_broadcaster_user_id"],
        ("channel.raid", "login") => &["/from_broadcaster_user_login"],
        ("channel.raid", "name") => &["/from_broadcaster_user_name"],
        ("channel.raid.outgoing", "id") => &["/to_broadcaster_user_id"],
        ("channel.raid.outgoing", "login") => &["/to_broadcaster_user_login"],
        ("channel.raid.outgoing", "name") => &["/to_broadcaster_user_name"],
        ("channel.shoutout.create", "id") => &["/to_broadcaster_user_id"],
        ("channel.shoutout.create", "login") => &["/to_broadcaster_user_login"],
        ("channel.shoutout.create", "name") => &["/to_broadcaster_user_name"],
        ("channel.shoutout.receive", "id") => &["/from_broadcaster_user_id"],
        ("channel.shoutout.receive", "login") => &["/from_broadcaster_user_login"],
        ("channel.shoutout.receive", "name") => &["/from_broadcaster_user_name"],
        ("channel.chat.notification", "id") => &["/chatter_user_id", "/user_id"],
        ("channel.chat.notification", "login") => &["/chatter_user_login", "/user_login"],
        ("channel.chat.notification", "name") => &["/chatter_user_name", "/user_name"],
        ("channel.chat.message", "id") => &["/chatter_user_id"],
        ("channel.chat.message", "login") => &["/chatter_user_login"],
        ("channel.chat.message", "name") => &["/chatter_user_name"],
        (_, "id") => &["/user_id", "/broadcaster_user_id"],
        (_, "login") => &["/user_login", "/broadcaster_user_login"],
        (_, "name") => &["/user_name", "/broadcaster_user_name"],
        _ => &[],
    }
}

fn secondary_user_name(event_type: &str, event: &Value) -> Option<String> {
    match event_type {
        "channel.raid" => string_at_any(event, &["/to_broadcaster_user_name"]),
        "channel.raid.outgoing" => string_at_any(event, &["/from_broadcaster_user_name"]),
        "channel.shoutout.create" => string_at_any(event, &["/broadcaster_user_name"]),
        "channel.shoutout.receive" => string_at_any(event, &["/broadcaster_user_name"]),
        "channel.chat.message" => string_at_any(event, &["/source_broadcaster_user_name"]),
        _ => None,
    }
}

fn string_at_any(value: &Value, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn number_at_any(value: &Value, paths: &[&str]) -> Option<f64> {
    paths.iter().find_map(|path| {
        let value = value.pointer(path)?;
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
    })
}

fn integer_at_any(value: &Value, paths: &[&str]) -> Option<u64> {
    paths.iter().find_map(|path| {
        let value = value.pointer(path)?;
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
            .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
    })
}

fn extract_message(event_type: &str, event: &Value) -> Option<String> {
    let paths: &[&str] = match event_type {
        "channel.subscription.message"
        | "channel.bits.use"
        | "channel.cheer"
        | "channel.chat.notification"
        | "channel.chat.message" => &["/message/text", "/message"],
        "channel.channel_points_custom_reward_redemption.add" => &["/user_input", "/reward/title"],
        "channel.channel_points_automatic_reward_redemption.add" => {
            &["/message/text", "/reward/title", "/reward/type"]
        }
        "channel.goal.begin" | "channel.goal.progress" | "channel.goal.end" => {
            &["/description", "/type"]
        }
        "channel.poll.begin"
        | "channel.poll.progress"
        | "channel.poll.end"
        | "channel.prediction.begin"
        | "channel.prediction.progress"
        | "channel.prediction.lock"
        | "channel.prediction.end" => &["/title"],
        "channel.charity_campaign.donate"
        | "channel.charity_campaign.start"
        | "channel.charity_campaign.progress"
        | "channel.charity_campaign.stop" => &["/charity_name", "/charity_description"],
        "channel.update" => &["/title", "/category_name"],
        _ => &[],
    };
    string_at_any(event, paths)
}

fn extract_amount(event_type: &str, event: &Value) -> (Option<f64>, Option<String>) {
    match event_type {
        "channel.bits.use" | "channel.cheer" => {
            (number_at_any(event, &["/bits"]), Some("BITS".into()))
        }
        "channel.charity_campaign.donate" => money_at(event, "/amount"),
        "channel.charity_campaign.start"
        | "channel.charity_campaign.progress"
        | "channel.charity_campaign.stop" => money_at(event, "/current_amount"),
        "channel.hype_train.begin" | "channel.hype_train.progress" => {
            (number_at_any(event, &["/total", "/progress"]), None)
        }
        "channel.goal.begin" | "channel.goal.progress" | "channel.goal.end" => {
            (number_at_any(event, &["/current_amount"]), None)
        }
        "channel.channel_points_custom_reward_redemption.add"
        | "channel.channel_points_automatic_reward_redemption.add" => (
            number_at_any(event, &["/reward/cost"]),
            Some("CHANNEL_POINTS".into()),
        ),
        _ => (None, None),
    }
}

fn money_at(event: &Value, base: &str) -> (Option<f64>, Option<String>) {
    let Some(money) = event.pointer(base) else {
        return (None, None);
    };
    let value = number_at_any(money, &["/value"]);
    let places = integer_at_any(money, &["/decimal_places"])
        .unwrap_or(0)
        .min(12);
    let amount = value.map(|value| value / 10_f64.powi(places as i32));
    let currency = string_at_any(money, &["/currency"]);
    (amount, currency)
}

fn extract_count(event_type: &str, event: &Value) -> Option<u64> {
    match event_type {
        "channel.subscription.gift" => integer_at_any(event, &["/total"]),
        "channel.subscription.message" => {
            integer_at_any(event, &["/cumulative_months", "/streak_months"])
        }
        "channel.raid" | "channel.raid.outgoing" => integer_at_any(event, &["/viewers"]),
        "channel.hype_train.begin" | "channel.hype_train.progress" | "channel.hype_train.end" => {
            integer_at_any(event, &["/level"])
        }
        "channel.ad_break.begin" => integer_at_any(event, &["/duration_seconds"]),
        "channel.chat.notification" => match event.get("notice_type").and_then(Value::as_str) {
            Some("watch_streak") => integer_at_any(event, &["/watch_streak/streak_count"]),
            Some("modiversary") => integer_at_any(event, &["/modiversary/months"]),
            Some("shared_chat_modiversary") => {
                integer_at_any(event, &["/shared_chat_modiversary/months"])
            }
            Some("bits_badge_tier") => integer_at_any(event, &["/bits_badge_tier/tier"]),
            Some("shared_chat_bits_badge_tier") => {
                integer_at_any(event, &["/shared_chat_bits_badge_tier/tier"])
            }
            _ => None,
        },
        _ => None,
    }
}

fn choice_votes(choice: &Value) -> Option<u64> {
    integer_at_any(choice, &["/votes"]).or_else(|| {
        let channel_points = integer_at_any(choice, &["/channel_points_votes"]).unwrap_or(0);
        let bits = integer_at_any(choice, &["/bits_votes"]).unwrap_or(0);
        (channel_points > 0 || bits > 0).then_some(channel_points.saturating_add(bits))
    })
}

fn poll_vote_totals(choices: &[Value]) -> Option<(u64, u64)> {
    let votes = choices.iter().filter_map(choice_votes).collect::<Vec<_>>();
    (!votes.is_empty()).then(|| {
        let winner = votes.iter().copied().max().unwrap_or(0);
        let total = votes.into_iter().fold(0_u64, u64::saturating_add);
        (winner, total)
    })
}

fn collect_common_details(event_type: &str, event: &Value, details: &mut BTreeMap<String, Value>) {
    const FIELDS: &[(&str, &str)] = &[
        ("level", "/level"),
        ("progress", "/progress"),
        ("goal", "/goal"),
        ("current", "/current_amount"),
        ("target", "/target_amount"),
        ("status", "/status"),
        ("type", "/type"),
        ("started_at", "/started_at"),
        ("ended_at", "/ended_at"),
        ("duration_seconds", "/duration_seconds"),
        ("is_automatic", "/is_automatic"),
        ("reward_id", "/reward/id"),
        ("reward_title", "/reward/title"),
        ("reward_prompt", "/reward/prompt"),
        ("reward_cost", "/reward/cost"),
        ("category_name", "/category_name"),
        ("language", "/language"),
        ("notice_type", "/notice_type"),
        ("color", "/color"),
    ];
    for (key, pointer) in FIELDS {
        if let Some(value) = event.pointer(pointer) {
            if !value.is_null() {
                details.insert((*key).into(), value.clone());
            }
        }
    }

    if event_type.starts_with("channel.poll.") {
        if let Some(choices) = event.get("choices") {
            details.insert("choices".into(), choices.clone());
            if let Some((winner_votes, total_votes)) = choices
                .as_array()
                .and_then(|choices| poll_vote_totals(choices))
            {
                details.insert("votes".into(), Value::from(winner_votes));
                details.insert("total_votes".into(), Value::from(total_votes));
            }
        }
    } else if event_type.starts_with("channel.prediction.") {
        if let Some(outcomes) = event.get("outcomes") {
            details.insert("outcomes".into(), outcomes.clone());
        }
        if let Some(winning) = event.get("winning_outcome_id") {
            if !winning.is_null() {
                details.insert("winning_outcome_id".into(), winning.clone());
            }
        }
    } else if event_type == "channel.bits.use" {
        if let Some(power_up) = event.get("power_up") {
            if !power_up.is_null() {
                details.insert("power_up".into(), power_up.clone());
            }
        }
    } else if event_type == "channel.chat.message" {
        for (key, pointer) in [
            ("source_broadcaster_user_id", "/source_broadcaster_user_id"),
            (
                "source_broadcaster_user_login",
                "/source_broadcaster_user_login",
            ),
            (
                "source_broadcaster_user_name",
                "/source_broadcaster_user_name",
            ),
            ("message_id", "/message_id"),
            ("source_message_id", "/source_message_id"),
        ] {
            if let Some(value) = event.pointer(pointer) {
                if !value.is_null() {
                    details.insert(key.into(), value.clone());
                }
            }
        }
    } else if event_type == "channel.chat.notification" {
        for (key, pointer) in [
            ("watch_streak", "/watch_streak/streak_count"),
            ("months", "/modiversary/months"),
            ("months", "/shared_chat_modiversary/months"),
            ("badge_threshold", "/bits_badge_tier/tier"),
            ("badge_threshold", "/shared_chat_bits_badge_tier/tier"),
        ] {
            if let Some(value) = event.pointer(pointer) {
                if !value.is_null() {
                    details.insert(key.into(), value.clone());
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TwitchConnectionState {
    Stopped,
    AuthorizationRequired,
    Connecting,
    Connected,
    Reconnecting,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TwitchStatus {
    pub state: TwitchConnectionState,
    pub connected: bool,
    pub display_name: Option<String>,
    pub granted_scopes: Vec<String>,
    pub websocket_state: String,
    pub subscription_count: usize,
    pub last_event_at: Option<String>,
    /// Unix epoch milliseconds, which the dashboard passes directly to
    /// JavaScript's `Date` constructor.
    pub last_validation_at: Option<i64>,
    pub error: Option<String>,
}

impl Default for TwitchStatus {
    fn default() -> Self {
        Self {
            state: TwitchConnectionState::Stopped,
            connected: false,
            display_name: None,
            granted_scopes: Vec::new(),
            websocket_state: "stopped".into(),
            subscription_count: 0,
            last_event_at: None,
            last_validation_at: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
pub enum TwitchServiceEvent {
    Status(TwitchStatus),
    Alert(TwitchAlert),
    SubscriptionError {
        key: String,
        event_type: String,
        message: String,
    },
    SubscriptionRevoked {
        event_type: String,
        status: String,
        reason: String,
    },
}

pub type TwitchEventSink = Arc<dyn Fn(TwitchServiceEvent) + Send + Sync + 'static>;

pub struct EventSubHandle {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl EventSubHandle {
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn is_stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub fn stop(mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for EventSubHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Starts a blocking EventSub WebSocket worker.  Dropping the handle requests a
/// stop; call [`EventSubHandle::stop`] when the caller wants to wait for it.
pub fn spawn_eventsub_service(
    client: TwitchClient,
    config: TwitchEventSubConfig,
    sink: TwitchEventSink,
) -> EventSubHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("twitch-eventsub".into())
        .spawn(move || run_eventsub_service(client, config, sink, thread_stop))
        .expect("failed to create Twitch EventSub thread");
    EventSubHandle {
        stop,
        worker: Some(worker),
    }
}

#[derive(Debug)]
struct MessageDeduper {
    capacity: usize,
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl MessageDeduper {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            ids: HashSet::with_capacity(capacity),
        }
    }

    /// True when this ID has not been seen.  Empty IDs are not deduplicated.
    fn accept(&mut self, id: &str) -> bool {
        if id.is_empty() || self.capacity == 0 {
            return true;
        }
        if self.ids.contains(id) {
            return false;
        }
        self.ids.insert(id.to_owned());
        self.order.push_back(id.to_owned());
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.ids.remove(&oldest);
            }
        }
        true
    }
}

fn run_eventsub_service(
    client: TwitchClient,
    config: TwitchEventSubConfig,
    sink: TwitchEventSink,
    stop: Arc<AtomicBool>,
) {
    let mut status = TwitchStatus::default();
    let mut deduper = MessageDeduper::new(2048);
    let mut reconnect_attempt = 0_u32;
    let mut next_url = EVENTSUB_WEBSOCKET_URL.to_owned();
    let mut transferring_session = false;

    if !config.broadcaster_user_id.is_empty() && !valid_twitch_id(&config.broadcaster_user_id) {
        status.state = TwitchConnectionState::Error;
        status.websocket_state = "error".into();
        status.error = Some("broadcaster user ID is invalid".into());
        emit_status(&sink, &status);
        return;
    }

    let enabled = config.normalized_keys();
    let known_keys: HashSet<&str> = CATALOG.iter().map(|entry| entry.key).collect();
    for unknown in config
        .enabled_subscriptions
        .iter()
        .filter(|key| !known_keys.contains(key.as_str()))
    {
        sink(TwitchServiceEvent::SubscriptionError {
            key: unknown.clone(),
            event_type: String::new(),
            message: "unsupported alert type".into(),
        });
    }

    while !stop.load(Ordering::Acquire) {
        let (token, identity) = match client.authorized_token() {
            Ok(value) => value,
            Err(TwitchError::NotAuthorized) | Err(TwitchError::Api { status: 401, .. }) => {
                status.state = TwitchConnectionState::AuthorizationRequired;
                status.connected = false;
                status.websocket_state = "authorization_required".into();
                status.error = Some("Connect Twitch to start alerts".into());
                emit_status(&sink, &status);
                if !sleep_interruptible(&stop, Duration::from_secs(3)) {
                    break;
                }
                continue;
            }
            Err(error) => {
                status.state = TwitchConnectionState::Error;
                status.connected = false;
                status.websocket_state = "error".into();
                status.error = Some(error.to_string());
                emit_status(&sink, &status);
                if !sleep_interruptible(&stop, Duration::from_secs(10)) {
                    break;
                }
                continue;
            }
        };

        let broadcaster_user_id = if config.broadcaster_user_id.is_empty() {
            identity.user_id.as_str()
        } else {
            config.broadcaster_user_id.as_str()
        };
        status.display_name = Some(identity.login.clone());
        status.granted_scopes = identity.scopes.clone();
        status.last_validation_at = Some(current_unix_millis());
        status.error = None;
        status.state = if reconnect_attempt == 0 {
            TwitchConnectionState::Connecting
        } else {
            TwitchConnectionState::Reconnecting
        };
        status.connected = false;
        status.websocket_state = if reconnect_attempt == 0 {
            "connecting".into()
        } else {
            "reconnecting".into()
        };
        emit_status(&sink, &status);

        let mut socket = match connect(next_url.as_str()) {
            Ok((mut socket, _)) => {
                if let Err(error) = set_websocket_read_timeout(&mut socket, Duration::from_secs(1))
                {
                    status.error = Some(format!("could not configure EventSub socket: {error}"));
                    emit_status(&sink, &status);
                }
                socket
            }
            Err(error) => {
                status.state = TwitchConnectionState::Reconnecting;
                status.websocket_state = "reconnecting".into();
                status.error = Some(websocket_error_message(&error));
                emit_status(&sink, &status);
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                next_url = EVENTSUB_WEBSOCKET_URL.into();
                transferring_session = false;
                if !sleep_interruptible(&stop, reconnect_delay(reconnect_attempt)) {
                    break;
                }
                continue;
            }
        };

        let welcome = match wait_for_welcome(&mut socket, &stop) {
            Ok(Some(welcome)) => welcome,
            Ok(None) => break,
            Err(error) => {
                status.state = TwitchConnectionState::Reconnecting;
                status.websocket_state = "reconnecting".into();
                status.error = Some(error.to_string());
                emit_status(&sink, &status);
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                next_url = EVENTSUB_WEBSOCKET_URL.into();
                transferring_session = false;
                if !sleep_interruptible(&stop, reconnect_delay(reconnect_attempt)) {
                    break;
                }
                continue;
            }
        };

        let session_id = welcome.session_id;
        let keepalive_timeout = Duration::from_secs(welcome.keepalive_timeout_seconds.max(10));
        let scope_set: HashSet<&str> = identity.scopes.iter().map(String::as_str).collect();
        let mut active_subscriptions = if transferring_session {
            status.subscription_count
        } else {
            0
        };

        if !transferring_session {
            for entry in &enabled {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                if let Some(required) = entry.required_scope {
                    if !scope_set.contains(required) {
                        sink(TwitchServiceEvent::SubscriptionError {
                            key: entry.key.into(),
                            event_type: entry.event_type.into(),
                            message: format!("missing Twitch permission {required}"),
                        });
                        continue;
                    }
                }

                let request = match event_subscription_request(
                    entry.key,
                    broadcaster_user_id,
                    &identity.user_id,
                ) {
                    Ok(request) => request,
                    Err(error) => {
                        sink(TwitchServiceEvent::SubscriptionError {
                            key: entry.key.into(),
                            event_type: entry.event_type.into(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                match client.create_subscription_with_token(
                    &token,
                    &identity,
                    &session_id,
                    &request,
                ) {
                    Ok(_) => active_subscriptions += 1,
                    Err(error) => sink(TwitchServiceEvent::SubscriptionError {
                        key: entry.key.into(),
                        event_type: entry.event_type.into(),
                        message: error.to_string(),
                    }),
                }
            }
        }

        reconnect_attempt = 0;
        transferring_session = false;
        next_url = EVENTSUB_WEBSOCKET_URL.into();
        status.state = TwitchConnectionState::Connected;
        status.connected = true;
        status.websocket_state = "connected".into();
        status.subscription_count = active_subscriptions;
        status.error = None;
        emit_status(&sink, &status);

        let mut last_message = Instant::now();
        let mut last_validation = Instant::now();
        let mut immediate_reconnect = false;

        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            match read_websocket_message(&mut socket) {
                Ok(SocketRead::Text(text)) => {
                    last_message = Instant::now();
                    let envelope: EventSubEnvelope = match serde_json::from_str(&text) {
                        Ok(envelope) => envelope,
                        Err(error) => {
                            status.error =
                                Some(format!("ignored an invalid EventSub message: {error}"));
                            emit_status(&sink, &status);
                            continue;
                        }
                    };
                    if !deduper.accept(&envelope.metadata.message_id) {
                        continue;
                    }
                    match envelope.metadata.message_type.as_str() {
                        "session_keepalive" | "session_welcome" => {}
                        "notification" => {
                            if let Some(alert) = normalize_notification(&envelope) {
                                status.last_event_at = Some(if alert.timestamp.is_empty() {
                                    unix_now().to_string()
                                } else {
                                    alert.timestamp.clone()
                                });
                                sink(TwitchServiceEvent::Alert(alert));
                                emit_status(&sink, &status);
                            }
                        }
                        "session_reconnect" => {
                            let reconnect_url = envelope
                                .payload
                                .pointer("/session/reconnect_url")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if valid_reconnect_url(reconnect_url) {
                                next_url = reconnect_url.to_owned();
                                transferring_session = true;
                                immediate_reconnect = true;
                            } else {
                                status.error =
                                    Some("Twitch sent an invalid reconnect address".into());
                                next_url = EVENTSUB_WEBSOCKET_URL.into();
                                transferring_session = false;
                            }
                            break;
                        }
                        "revocation" => {
                            active_subscriptions = active_subscriptions.saturating_sub(1);
                            status.subscription_count = active_subscriptions;
                            let subscription =
                                envelope.payload.get("subscription").unwrap_or(&Value::Null);
                            let event_type = subscription
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or(&envelope.metadata.subscription_type)
                                .to_owned();
                            let reason = subscription
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("revoked")
                                .to_owned();
                            sink(TwitchServiceEvent::SubscriptionRevoked {
                                event_type,
                                status: reason.clone(),
                                reason,
                            });
                            emit_status(&sink, &status);
                        }
                        _ => {}
                    }
                }
                Ok(SocketRead::Activity) => {
                    last_message = Instant::now();
                }
                Ok(SocketRead::TimedOut) => {}
                Ok(SocketRead::Closed) => break,
                Err(error) => {
                    status.error = Some(websocket_error_message(&error));
                    break;
                }
            }

            if last_message.elapsed() > keepalive_timeout + Duration::from_secs(10) {
                status.error = Some("Twitch EventSub keepalive timed out".into());
                break;
            }

            if last_validation.elapsed() >= VALIDATE_EVERY {
                match client.ensure_authorized() {
                    Ok(validated) => {
                        status.display_name = Some(validated.login);
                        status.granted_scopes = validated.scopes;
                        status.last_validation_at = Some(current_unix_millis());
                        emit_status(&sink, &status);
                        last_validation = Instant::now();
                    }
                    Err(error) => {
                        status.error = Some(error.to_string());
                        break;
                    }
                }
            }
        }

        status.connected = false;
        status.state = TwitchConnectionState::Reconnecting;
        status.websocket_state = "reconnecting".into();
        emit_status(&sink, &status);
        if stop.load(Ordering::Acquire) {
            break;
        }
        if immediate_reconnect {
            reconnect_attempt = 0;
        } else {
            next_url = EVENTSUB_WEBSOCKET_URL.into();
            transferring_session = false;
            reconnect_attempt = reconnect_attempt.saturating_add(1);
            if !sleep_interruptible(&stop, reconnect_delay(reconnect_attempt)) {
                break;
            }
        }
    }

    status.state = TwitchConnectionState::Stopped;
    status.connected = false;
    status.websocket_state = "stopped".into();
    status.subscription_count = 0;
    status.error = None;
    emit_status(&sink, &status);
}

fn emit_status(sink: &TwitchEventSink, status: &TwitchStatus) {
    sink(TwitchServiceEvent::Status(status.clone()));
}

fn sleep_interruptible(stop: &AtomicBool, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    !stop.load(Ordering::Acquire)
}

fn reconnect_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(5);
    Duration::from_secs(1_u64 << exponent).min(MAX_RECONNECT_DELAY)
}

struct SessionWelcome {
    session_id: String,
    keepalive_timeout_seconds: u64,
}

fn wait_for_welcome(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    stop: &AtomicBool,
) -> Result<Option<SessionWelcome>, TwitchError> {
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Ok(None);
        }
        match read_websocket_message(socket) {
            Ok(SocketRead::Text(text)) => {
                let envelope: EventSubEnvelope = serde_json::from_str(&text)?;
                if envelope.metadata.message_type != "session_welcome" {
                    continue;
                }
                let session_id = envelope
                    .payload
                    .pointer("/session/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        TwitchError::Protocol("EventSub welcome omitted session ID".into())
                    })?
                    .to_owned();
                let keepalive_timeout_seconds = envelope
                    .payload
                    .pointer("/session/keepalive_timeout_seconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(10);
                return Ok(Some(SessionWelcome {
                    session_id,
                    keepalive_timeout_seconds,
                }));
            }
            Ok(SocketRead::Activity | SocketRead::TimedOut) => {}
            Ok(SocketRead::Closed) => {
                return Err(TwitchError::Protocol(
                    "EventSub closed before the session welcome".into(),
                ))
            }
            Err(error) => return Err(TwitchError::WebSocket(websocket_error_message(&error))),
        }
    }
    Err(TwitchError::Protocol(
        "EventSub did not send a session welcome in time".into(),
    ))
}

enum SocketRead {
    Text(String),
    Activity,
    TimedOut,
    Closed,
}

fn read_websocket_message(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
) -> Result<SocketRead, WebSocketError> {
    match socket.read() {
        Ok(Message::Text(text)) => Ok(SocketRead::Text(text.as_str().to_owned())),
        Ok(Message::Close(_)) => Ok(SocketRead::Closed),
        Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_)) => {
            Ok(SocketRead::Activity)
        }
        Err(WebSocketError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            Ok(SocketRead::TimedOut)
        }
        Err(WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed) => {
            Ok(SocketRead::Closed)
        }
        Err(error) => Err(error),
    }
}

fn set_websocket_read_timeout(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(timeout)),
        MaybeTlsStream::Rustls(stream) => stream.sock.set_read_timeout(Some(timeout)),
        _ => Ok(()),
    }
}

fn valid_reconnect_url(url: &str) -> bool {
    url.starts_with(EVENTSUB_RECONNECT_PREFIX) && !url.contains(['\r', '\n']) && url.len() <= 4096
}

fn websocket_error_message(error: &WebSocketError) -> String {
    match error {
        WebSocketError::Io(error) => format!("network error: {error}"),
        WebSocketError::Tls(_) => "TLS connection failed".into(),
        WebSocketError::Http(response) => {
            format!("EventSub handshake returned HTTP {}", response.status())
        }
        WebSocketError::HttpFormat(_) => "EventSub handshake was invalid".into(),
        WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed => {
            "EventSub connection closed".into()
        }
        WebSocketError::Capacity(_) => "EventSub message was too large".into(),
        WebSocketError::Protocol(_) => "EventSub protocol error".into(),
        WebSocketError::Utf8(_) => "EventSub sent invalid text".into(),
        WebSocketError::WriteBufferFull(_) => "EventSub write buffer is full".into(),
        WebSocketError::AttackAttempt => "EventSub rejected an unsafe message".into(),
        _ => "EventSub connection failed".into(),
    }
}

/// Stable settings key used by the dashboard.  Several EventSub lifecycle
/// variants intentionally share one visual-alert configuration.
pub fn logical_alert_key(kind: TwitchAlertKind) -> &'static str {
    match kind {
        TwitchAlertKind::Follow => "follow",
        TwitchAlertKind::Subscription => "new_sub",
        TwitchAlertKind::Resubscription => "resub",
        TwitchAlertKind::GiftSubscription => "sub_gift",
        TwitchAlertKind::Cheer => "bits",
        TwitchAlertKind::PowerUp => "power_up",
        TwitchAlertKind::Raid => "raid",
        TwitchAlertKind::OutgoingRaid => "outgoing_raid",
        TwitchAlertKind::RewardRedemption => "channel_points",
        TwitchAlertKind::AutomaticReward => "automatic_points",
        TwitchAlertKind::CharityDonation => "charity_donation",
        TwitchAlertKind::HypeTrainBegin
        | TwitchAlertKind::HypeTrainProgress
        | TwitchAlertKind::HypeTrainEnd => "hype_train",
        TwitchAlertKind::GoalBegin | TwitchAlertKind::GoalProgress | TwitchAlertKind::GoalEnd => {
            "goal"
        }
        TwitchAlertKind::PollBegin | TwitchAlertKind::PollProgress | TwitchAlertKind::PollEnd => {
            "poll"
        }
        TwitchAlertKind::PredictionBegin
        | TwitchAlertKind::PredictionProgress
        | TwitchAlertKind::PredictionLock
        | TwitchAlertKind::PredictionEnd => "prediction",
        TwitchAlertKind::CharityCampaignStart
        | TwitchAlertKind::CharityCampaignProgress
        | TwitchAlertKind::CharityCampaignStop => "charity_campaign",
        TwitchAlertKind::ShoutoutCreated | TwitchAlertKind::ShoutoutReceived => "shoutout",
        TwitchAlertKind::StreamOnline => "stream_online",
        TwitchAlertKind::StreamOffline => "stream_offline",
        TwitchAlertKind::AdBreak => "ad_break",
        TwitchAlertKind::ChannelUpdate => "channel_update",
        TwitchAlertKind::ChatUpgrade => "sub_upgrade",
        TwitchAlertKind::PayItForward => "pay_it_forward",
        TwitchAlertKind::WatchStreak => "watch_streak",
        TwitchAlertKind::Modiversary => "modiversary",
        TwitchAlertKind::BitsBadge => "bits_badge",
        TwitchAlertKind::Announcement => "chat_announcement",
        TwitchAlertKind::UserIntro => "user_intro",
        TwitchAlertKind::SharedChat => "shared_chat",
    }
}

/// Builds a representative payload for the Twitch tab's Test button without a
/// network request.  It accepts the aggregate settings keys exposed by
/// [`logical_alert_key`].
pub fn sample_alert_for_test(key: &str) -> Option<TwitchAlert> {
    let (kind, title, amount, currency, count, message) = match key {
        "follow" => (
            TwitchAlertKind::Follow,
            "New follower",
            None,
            None,
            None,
            None,
        ),
        "new_sub" => (
            TwitchAlertKind::Subscription,
            "New subscriber",
            None,
            None,
            None,
            Some("Welcome to the community!"),
        ),
        "resub" => (
            TwitchAlertKind::Resubscription,
            "Resubscription",
            None,
            None,
            Some(12),
            Some("One year!"),
        ),
        "sub_gift" => (
            TwitchAlertKind::GiftSubscription,
            "Gift subscriptions",
            None,
            None,
            Some(5),
            Some("Five gifted subs"),
        ),
        "bits" => (
            TwitchAlertKind::Cheer,
            "Cheer",
            Some(100.0),
            Some("BITS"),
            None,
            Some("Cheer100"),
        ),
        "power_up" => (
            TwitchAlertKind::PowerUp,
            "Power-up",
            Some(100.0),
            Some("BITS"),
            None,
            Some("Celebration On-Screen"),
        ),
        "raid" => (
            TwitchAlertKind::Raid,
            "Incoming raid",
            None,
            None,
            Some(42),
            Some("Raid incoming"),
        ),
        "outgoing_raid" => (
            TwitchAlertKind::OutgoingRaid,
            "Outgoing raid",
            None,
            None,
            Some(42),
            Some("Sending the party onward"),
        ),
        "channel_points" => (
            TwitchAlertKind::RewardRedemption,
            "Channel-point reward",
            Some(1_000.0),
            Some("CHANNEL_POINTS"),
            None,
            Some("Hydrate"),
        ),
        "automatic_points" => (
            TwitchAlertKind::AutomaticReward,
            "Automatic reward",
            Some(500.0),
            Some("CHANNEL_POINTS"),
            None,
            Some("Highlight My Message"),
        ),
        "charity_donation" => (
            TwitchAlertKind::CharityDonation,
            "Charity donation",
            Some(25.0),
            Some("USD"),
            None,
            Some("For a great cause"),
        ),
        "hype_train" => (
            TwitchAlertKind::HypeTrainProgress,
            "Hype Train progress",
            Some(2_500.0),
            None,
            Some(3),
            Some("Level 3"),
        ),
        "goal" => (
            TwitchAlertKind::GoalProgress,
            "Creator goal progress",
            Some(75.0),
            None,
            None,
            Some("75 / 100"),
        ),
        "poll" => (
            TwitchAlertKind::PollEnd,
            "Poll ended",
            None,
            None,
            None,
            Some("Frost Orb wins!"),
        ),
        "prediction" => (
            TwitchAlertKind::PredictionEnd,
            "Prediction ended",
            None,
            None,
            None,
            Some("Boss defeated"),
        ),
        "charity_campaign" => (
            TwitchAlertKind::CharityCampaignProgress,
            "Charity campaign progress",
            Some(750.0),
            Some("USD"),
            None,
            Some("75% of goal"),
        ),
        "shoutout" => (
            TwitchAlertKind::ShoutoutReceived,
            "Shoutout received",
            None,
            None,
            Some(88),
            Some("Thanks for the shoutout!"),
        ),
        "stream_online" => (
            TwitchAlertKind::StreamOnline,
            "Stream online",
            None,
            None,
            None,
            Some("HS Tracker is live"),
        ),
        "stream_offline" => (
            TwitchAlertKind::StreamOffline,
            "Stream offline",
            None,
            None,
            None,
            None,
        ),
        "ad_break" => (
            TwitchAlertKind::AdBreak,
            "Ad break started",
            None,
            None,
            Some(60),
            Some("60 seconds"),
        ),
        "channel_update" => (
            TwitchAlertKind::ChannelUpdate,
            "Channel updated",
            None,
            None,
            None,
            Some("Hero Siege · Frost Orb"),
        ),
        "sub_upgrade" => (
            TwitchAlertKind::ChatUpgrade,
            "Subscription upgraded",
            None,
            None,
            None,
            Some("Upgraded to a paid sub"),
        ),
        "pay_it_forward" => (
            TwitchAlertKind::PayItForward,
            "Pay it forward",
            None,
            None,
            None,
            Some("Gift paid forward"),
        ),
        "chat_announcement" => (
            TwitchAlertKind::Announcement,
            "Announcement",
            None,
            None,
            None,
            Some("Welcome, everyone!"),
        ),
        "watch_streak" => (
            TwitchAlertKind::WatchStreak,
            "Watch streak",
            None,
            None,
            Some(10),
            Some("10 streams in a row"),
        ),
        "modiversary" => (
            TwitchAlertKind::Modiversary,
            "Mod anniversary",
            None,
            None,
            Some(24),
            Some("24 months as a mod"),
        ),
        "bits_badge" => (
            TwitchAlertKind::BitsBadge,
            "Bits badge unlocked",
            None,
            None,
            Some(10_000),
            Some("New Bits badge"),
        ),
        "user_intro" => (
            TwitchAlertKind::UserIntro,
            "First-time chatter",
            None,
            None,
            None,
            Some("Hello, chat!"),
        ),
        "shared_chat" => (
            TwitchAlertKind::SharedChat,
            "Shared Chat",
            None,
            None,
            None,
            Some("Hello from the partner channel!"),
        ),
        _ => return None,
    };

    let mut details = BTreeMap::new();
    let secondary_user_name = match key {
        "hype_train" => {
            details.insert("level".into(), Value::from(3));
            details.insert("progress".into(), Value::from(2_500));
            details.insert("goal".into(), Value::from(5_000));
            None
        }
        "poll" => {
            details.insert("votes".into(), Value::from(70));
            details.insert("total_votes".into(), Value::from(100));
            details.insert(
                "choices".into(),
                json!([
                    { "id": "frost", "title": "Frost Orb", "votes": 70 },
                    { "id": "fire", "title": "Fire Orb", "votes": 30 }
                ]),
            );
            None
        }
        "watch_streak" => {
            details.insert("watch_streak".into(), Value::from(10));
            None
        }
        "modiversary" => {
            details.insert("months".into(), Value::from(24));
            None
        }
        "bits_badge" => {
            details.insert("badge_threshold".into(), Value::from(10_000));
            None
        }
        "shared_chat" => {
            details.insert(
                "source_broadcaster_user_name".into(),
                Value::String("PartnerChannel".into()),
            );
            Some("PartnerChannel".into())
        }
        _ => None,
    };

    Some(TwitchAlert {
        id: format!("test-{key}-{}", unix_now()),
        kind,
        source_type: "test".into(),
        timestamp: unix_now().to_string(),
        title: title.into(),
        user_id: Some("00000000".into()),
        user_login: Some("testviewer".into()),
        user_name: Some("TestViewer".into()),
        secondary_user_name,
        message: message.map(str::to_owned),
        amount,
        currency: currency.map(str::to_owned),
        count,
        tier: matches!(
            kind,
            TwitchAlertKind::Subscription
                | TwitchAlertKind::Resubscription
                | TwitchAlertKind::GiftSubscription
        )
        .then(|| "1000".into()),
        anonymous: false,
        details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(event_type: &str, event: Value) -> String {
        notification_with_condition(event_type, json!({}), event)
    }

    fn notification_with_condition(event_type: &str, condition: Value, event: Value) -> String {
        json!({
            "metadata": {
                "message_id": "message-1",
                "message_type": "notification",
                "message_timestamp": "2026-08-27T12:00:00Z",
                "subscription_type": event_type,
                "subscription_version": "1"
            },
            "payload": {
                "subscription": { "type": event_type, "condition": condition },
                "event": event
            }
        })
        .to_string()
    }

    #[test]
    fn catalog_has_unique_keys_and_current_special_versions() {
        let catalog = twitch_alert_catalog();
        assert!(catalog.len() >= 32);
        let keys: HashSet<&str> = catalog.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys.len(), catalog.len());
        assert_eq!(
            catalog
                .iter()
                .find(|entry| entry.key == "follow")
                .unwrap()
                .version,
            "2"
        );
        assert_eq!(
            catalog
                .iter()
                .find(|entry| entry.key == "automatic_reward")
                .unwrap()
                .version,
            "2"
        );
        assert_eq!(
            catalog
                .iter()
                .find(|entry| entry.key == "hype_train_begin")
                .unwrap()
                .version,
            "2"
        );
        assert!(catalog
            .iter()
            .any(|entry| entry.event_type == "channel.bits.use"));
        assert!(!catalog
            .iter()
            .any(|entry| entry.event_type == "channel.cheer"));
        assert!(catalog.iter().any(|entry| entry.key == "outgoing_raid"));
        assert!(catalog.iter().any(|entry| entry.key == "shared_chat"));
    }

    #[test]
    fn catalog_scopes_are_sorted_and_deduplicated() {
        let scopes = all_supported_scopes();
        assert!(scopes.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(scopes.contains(&"moderator:read:followers".into()));
        assert!(scopes.contains(&"channel:read:subscriptions".into()));
        assert!(scopes.contains(&"bits:read".into()));
        assert!(scopes.contains(&"user:read:chat".into()));
    }

    #[test]
    fn logical_plan_deduplicates_shared_eventsub_transports() {
        let keys = subscription_keys_for_logical_alerts([
            "bits",
            "power_up",
            "chat_announcement",
            "sub_upgrade",
            "hype_train",
            "outgoing_raid",
            "shared_chat",
        ]);
        assert_eq!(keys.iter().filter(|key| *key == "bits").count(), 1);
        assert_eq!(
            keys.iter().filter(|key| *key == "chat_milestones").count(),
            1
        );
        assert!(keys.contains(&"hype_train_begin".into()));
        assert!(keys.contains(&"hype_train_progress".into()));
        assert!(keys.contains(&"hype_train_end".into()));
        assert!(keys.contains(&"outgoing_raid".into()));
        assert!(keys.contains(&"shared_chat".into()));

        let scopes = required_scopes_for_logical_alerts(["bits", "power_up", "shared_chat"]);
        assert_eq!(scopes, vec!["bits:read", "user:read:chat"]);
    }

    #[test]
    fn conditions_use_authenticated_user_for_follow_and_chat() {
        let follow = event_subscription_request("follow", "123", "123").unwrap();
        assert_eq!(follow.condition["broadcaster_user_id"], "123");
        assert_eq!(follow.condition["moderator_user_id"], "123");

        let chat = event_subscription_request("chat_milestones", "123", "123").unwrap();
        assert_eq!(chat.condition["user_id"], "123");

        let raid = event_subscription_request("raid", "123", "123").unwrap();
        assert_eq!(raid.condition.len(), 1);
        assert_eq!(raid.condition["to_broadcaster_user_id"], "123");

        let outgoing = event_subscription_request("outgoing_raid", "123", "123").unwrap();
        assert_eq!(outgoing.condition.len(), 1);
        assert_eq!(outgoing.condition["from_broadcaster_user_id"], "123");
    }

    #[test]
    fn maps_follow_notification() {
        let alert = normalize_eventsub_message(&notification(
            "channel.follow",
            json!({
                "user_id": "7",
                "user_login": "frostfan",
                "user_name": "FrostFan",
                "broadcaster_user_id": "123",
                "followed_at": "2026-08-27T12:00:00Z"
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(alert.kind, TwitchAlertKind::Follow);
        assert_eq!(alert.user_name.as_deref(), Some("FrostFan"));
        assert_eq!(logical_alert_key(alert.kind), "follow");
    }

    #[test]
    fn suppresses_gift_recipient_subscription_duplicate() {
        let alert = normalize_eventsub_message(&notification(
            "channel.subscribe",
            json!({
                "user_id": "7",
                "user_name": "GiftRecipient",
                "is_gift": true,
                "tier": "1000"
            }),
        ))
        .unwrap();
        assert!(alert.is_none());
    }

    #[test]
    fn maps_bits_power_up_separately_from_cheer() {
        let alert = normalize_eventsub_message(&notification(
            "channel.bits.use",
            json!({
                "user_id": "7",
                "user_name": "BitsFan",
                "bits": 450,
                "type": "power_up",
                "power_up": { "type": "celebration" },
                "message": { "text": "boom" }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(alert.kind, TwitchAlertKind::PowerUp);
        assert_eq!(alert.amount, Some(450.0));
        assert_eq!(alert.currency.as_deref(), Some("BITS"));
        assert_eq!(alert.message.as_deref(), Some("boom"));
    }

    #[test]
    fn hype_train_retains_level_separately_from_contribution_total() {
        let alert = normalize_eventsub_message(&notification(
            "channel.hype_train.progress",
            json!({
                "broadcaster_user_id": "123",
                "level": 4,
                "total": 8200,
                "progress": 1200,
                "goal": 2500
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(alert.kind, TwitchAlertKind::HypeTrainProgress);
        assert_eq!(alert.count, Some(4));
        assert_eq!(alert.amount, Some(8200.0));
        assert_eq!(alert.details.get("level"), Some(&Value::from(4)));
    }

    #[test]
    fn poll_retains_leading_and_total_vote_counts_without_double_counting() {
        let alert = normalize_eventsub_message(&notification(
            "channel.poll.end",
            json!({
                "title": "Choose a build",
                "status": "completed",
                "choices": [
                    {
                        "id": "frost",
                        "title": "Frost Orb",
                        "votes": 10,
                        "channel_points_votes": 4,
                        "bits_votes": 2
                    },
                    {
                        "id": "fire",
                        "title": "Fire Orb",
                        "votes": 6,
                        "channel_points_votes": 6,
                        "bits_votes": 0
                    }
                ]
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(alert.details.get("votes"), Some(&Value::from(10)));
        assert_eq!(alert.details.get("total_votes"), Some(&Value::from(16)));
        assert!(alert.details.get("choices").is_some_and(Value::is_array));
    }

    #[test]
    fn chat_milestones_expose_threshold_counts_and_named_details() {
        let watch = normalize_eventsub_message(&notification(
            "channel.chat.notification",
            json!({
                "notice_type": "watch_streak",
                "chatter_user_name": "Viewer",
                "watch_streak": { "streak_count": 8, "channel_points_awarded": 450 }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(watch.kind, TwitchAlertKind::WatchStreak);
        assert_eq!(watch.count, Some(8));
        assert_eq!(watch.details.get("watch_streak"), Some(&Value::from(8)));

        let modiversary = normalize_eventsub_message(&notification(
            "channel.chat.notification",
            json!({
                "notice_type": "modiversary",
                "chatter_user_name": "ModViewer",
                "modiversary": { "months": 18 }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(modiversary.kind, TwitchAlertKind::Modiversary);
        assert_eq!(modiversary.count, Some(18));
        assert_eq!(modiversary.details.get("months"), Some(&Value::from(18)));

        let badge = normalize_eventsub_message(&notification(
            "channel.chat.notification",
            json!({
                "notice_type": "bits_badge_tier",
                "chatter_user_name": "Cheerer",
                "bits_badge_tier": { "tier": 10000 }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(badge.kind, TwitchAlertKind::BitsBadge);
        assert_eq!(badge.count, Some(10_000));
        assert_eq!(
            badge.details.get("badge_threshold"),
            Some(&Value::from(10_000))
        );
    }

    #[test]
    fn distinguishes_incoming_and_outgoing_raids_by_subscription_condition() {
        let outgoing = normalize_eventsub_message(&notification_with_condition(
            "channel.raid",
            json!({ "from_broadcaster_user_id": "123" }),
            json!({
                "from_broadcaster_user_id": "123",
                "from_broadcaster_user_name": "Kewk",
                "to_broadcaster_user_id": "456",
                "to_broadcaster_user_login": "nextchannel",
                "to_broadcaster_user_name": "NextChannel",
                "viewers": 42
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(outgoing.kind, TwitchAlertKind::OutgoingRaid);
        assert_eq!(outgoing.user_name.as_deref(), Some("NextChannel"));
        assert_eq!(outgoing.secondary_user_name.as_deref(), Some("Kewk"));
        assert_eq!(outgoing.count, Some(42));
        assert_eq!(logical_alert_key(outgoing.kind), "outgoing_raid");
    }

    #[test]
    fn shared_chat_only_maps_messages_from_another_source_channel() {
        let ordinary = normalize_eventsub_message(&notification(
            "channel.chat.message",
            json!({
                "broadcaster_user_id": "123",
                "source_broadcaster_user_id": "123",
                "chatter_user_id": "7",
                "chatter_user_name": "Viewer",
                "message": { "text": "ordinary message" }
            }),
        ))
        .unwrap();
        assert!(ordinary.is_none());

        let shared = normalize_eventsub_message(&notification(
            "channel.chat.message",
            json!({
                "broadcaster_user_id": "123",
                "source_broadcaster_user_id": "999",
                "source_broadcaster_user_login": "partner",
                "source_broadcaster_user_name": "PartnerChannel",
                "chatter_user_id": "7",
                "chatter_user_login": "viewer",
                "chatter_user_name": "Viewer",
                "message": { "text": "hello from over here" }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(shared.kind, TwitchAlertKind::SharedChat);
        assert_eq!(shared.user_name.as_deref(), Some("Viewer"));
        assert_eq!(
            shared.secondary_user_name.as_deref(),
            Some("PartnerChannel")
        );
        assert_eq!(shared.message.as_deref(), Some("hello from over here"));
        assert_eq!(
            shared
                .details
                .get("source_broadcaster_user_name")
                .and_then(Value::as_str),
            Some("PartnerChannel")
        );
        assert_eq!(logical_alert_key(shared.kind), "shared_chat");
    }

    #[test]
    fn charity_minor_units_become_decimal_amount() {
        let alert = normalize_eventsub_message(&notification(
            "channel.charity_campaign.donate",
            json!({
                "user_id": "7",
                "user_name": "Donor",
                "charity_name": "Example Fund",
                "amount": { "value": 12345, "decimal_places": 2, "currency": "USD" }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(alert.kind, TwitchAlertKind::CharityDonation);
        assert_eq!(alert.amount, Some(123.45));
        assert_eq!(alert.currency.as_deref(), Some("USD"));
    }

    #[test]
    fn chat_overlap_is_suppressed_but_milestone_is_mapped() {
        let duplicate = normalize_eventsub_message(&notification(
            "channel.chat.notification",
            json!({ "notice_type": "resub", "chatter_user_name": "Viewer" }),
        ))
        .unwrap();
        assert!(duplicate.is_none());

        let milestone = normalize_eventsub_message(&notification(
            "channel.chat.notification",
            json!({
                "notice_type": "watch_streak",
                "chatter_user_id": "7",
                "chatter_user_name": "Viewer",
                "message": { "text": "10 streams!" }
            }),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(milestone.kind, TwitchAlertKind::WatchStreak);
        assert_eq!(logical_alert_key(milestone.kind), "watch_streak");
    }

    #[test]
    fn deduper_rejects_replays_and_evicts_old_ids() {
        let mut deduper = MessageDeduper::new(2);
        assert!(deduper.accept("a"));
        assert!(!deduper.accept("a"));
        assert!(deduper.accept("b"));
        assert!(deduper.accept("c"));
        assert!(deduper.accept("a"));
        assert!(deduper.accept(""));
        assert!(deduper.accept(""));
    }

    #[test]
    fn sample_alert_accepts_dashboard_aggregate_keys() {
        for key in [
            "follow",
            "new_sub",
            "sub_gift",
            "bits",
            "channel_points",
            "hype_train",
            "chat_announcement",
            "sub_upgrade",
            "outgoing_raid",
            "shared_chat",
        ] {
            let alert = sample_alert_for_test(key).unwrap();
            assert_eq!(logical_alert_key(alert.kind), key);
        }
        assert!(sample_alert_for_test("not-real").is_none());
    }

    #[test]
    fn validation_timestamp_serializes_as_javascript_epoch_milliseconds() {
        let timestamp = current_unix_millis();
        assert!(timestamp >= 1_000_000_000_000);
        let status = TwitchStatus {
            last_validation_at: Some(timestamp),
            ..TwitchStatus::default()
        };
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["last_validation_at"].as_i64(), Some(timestamp));
    }
}
