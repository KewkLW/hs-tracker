//! Privacy-bounded market protocol observations.
//!
//! This module records structure, not packet bodies. It is intentionally
//! separate from the raw debug capture because client messages carry account
//! identifiers, session credentials, checksums, fingerprints and item masks.

use std::collections::{hash_map::RandomState, BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::parser::Flow;

static WRITE_LOCK: Mutex<()> = Mutex::new(());
const OBSERVATION_KEEP: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct Port443Summary {
    pub packet_count: u64,
    pub payload_bytes: u64,
    pub tls_like_packet_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarketObservation {
    pub direction: &'static str,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_name: Option<String>,
    pub structural_fields: Vec<&'static str>,
    pub redacted_fields: Vec<&'static str>,
}

#[derive(Serialize)]
struct ObservationRecord<'a> {
    observed_at_unix_ms: u64,
    record_type: &'static str,
    flow_tag: String,
    adapter_tag: String,
    #[serde(flatten)]
    observation: &'a MarketObservation,
}

#[derive(Serialize)]
struct Port443WindowRecord {
    observed_at_unix_ms: u64,
    record_type: &'static str,
    flow_tag: String,
    adapter_tag: String,
    packet_count: u64,
    payload_bytes: u64,
    tls_like_packet_count: u64,
    payload_recorded: bool,
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64
}

fn flat_key(key: &str) -> String {
    key.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

fn opaque_tag(value: &str) -> String {
    // Secret-keyed once per process: records from one controlled experiment
    // can be correlated without leaving an address, port tuple, or device GUID
    // that a shared log could disclose. Tags deliberately change after restart.
    static TAG_STATE: OnceLock<RandomState> = OnceLock::new();
    let state = TAG_STATE.get_or_init(RandomState::new);
    let mut hasher = state.build_hasher();
    hasher.write(value.as_bytes());
    format!("{:016x}", hasher.finish())
}

fn flow_tag(flow: Flow) -> String {
    opaque_tag(&format!("{}:{}:{}", flow.0, flow.1, flow.2))
}

fn rotate_if_needed(path: &Path, keep: u64) -> io::Result<()> {
    if std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0) < keep {
        return Ok(());
    }

    let old = path.with_extension("old.jsonl");
    match std::fs::remove_file(&old) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::rename(path, old)
}

fn observation_writer(path: &Path) -> io::Result<BufWriter<std::fs::File>> {
    rotate_if_needed(path, OBSERVATION_KEEP)?;
    OpenOptions::new().create(true).append(true).open(path).map(BufWriter::new)
}

fn structural_name(key: &str) -> Option<&'static str> {
    match flat_key(key).as_str() {
        "route" => Some("route"),
        "status" => Some("status"),
        "message" => Some("message"),
        "itemdata" => Some("item_data"),
        "itemname" => Some("item_name"),
        "itemmask" => Some("item_mask"),
        "marketid" => Some("market_id"),
        "search" | "searchterm" => Some("search"),
        "query" => Some("query"),
        "price" | "askingprice" | "buyout" => Some("price"),
        "cost" => Some("cost"),
        "currency" | "currencytype" => Some("currency"),
        "page" | "pagenumber" => Some("page"),
        "result" | "results" | "listing" | "listings" | "items" => Some("results"),
        "total" | "count" | "itemcount" => Some("count"),
        "operationtime" => Some("operation_time"),
        _ => None,
    }
}

fn sensitive_name(key: &str) -> Option<&'static str> {
    match flat_key(key).as_str() {
        "identifier" => Some("identifier"),
        "checksum" => Some("checksum"),
        "accountid" => Some("account_id"),
        "uniqueaccountid" => Some("unique_account_id"),
        "crossregionidentifier" => Some("crossregion_identifier"),
        "fingerprint" => Some("fingerprint"),
        "hash" | "itemhash" | "sh" => Some("item_hash"),
        "itemmask" => Some("item_mask"),
        _ => None,
    }
}

fn safe_text(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).take(max_chars).collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn route_of(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    object
        .iter()
        .find(|(key, _)| matches!(flat_key(key).as_str(), "route"))
        .and_then(|(_, value)| safe_text(Some(value), 120))
        .map(|route| route.split(['?', '#']).next().unwrap_or_default().trim_matches('/').to_string())
        .filter(|route| !route.is_empty())
        .map(|route| {
            let lower = route.to_ascii_lowercase();
            match lower.as_str() {
                // A route already observed publicly and known to contain no
                // account-specific path segment.
                "market/market_player_get_items_on_sale" => lower,
                _ if lower.contains("market") => "market/<redacted>".to_string(),
                _ => "<redacted>".to_string(),
            }
        })
}

fn item_name_of(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    object.iter().find(|(key, _)| flat_key(key) == "itemname").and_then(|(_, value)| safe_text(Some(value), 160))
}

fn collect_keys(value: &Value, structural: &mut BTreeSet<&'static str>, sensitive: &mut BTreeSet<&'static str>, depth: usize) {
    if depth > 4 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, nested) in object {
                if let Some(name) = structural_name(key) {
                    structural.insert(name);
                }
                if let Some(name) = sensitive_name(key) {
                    sensitive.insert(name);
                }
                collect_keys(nested, structural, sensitive, depth + 1);
            }
        }
        Value::Array(values) => {
            for nested in values.iter().take(100) {
                collect_keys(nested, structural, sensitive, depth + 1);
            }
        }
        _ => {}
    }
}

fn message_mentions_market(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object
        .iter()
        .any(|(key, value)| flat_key(key) == "message" && value.as_str().is_some_and(|text| text.to_ascii_lowercase().contains("market")))
}

fn observation(value: &Value) -> Option<MarketObservation> {
    if !value.is_object() {
        return None;
    }
    let mut structural = BTreeSet::new();
    let mut sensitive = BTreeSet::new();
    collect_keys(value, &mut structural, &mut sensitive, 0);
    let route = route_of(value);
    let route_is_market = route.as_deref().is_some_and(|r| r.contains("market"));
    let client_envelope = sensitive.contains("identifier") || sensitive.contains("checksum");
    let auction_post = client_envelope && structural.contains("item_data") && structural.contains("item_name") && structural.contains("item_mask");
    let listing_shape = structural.contains("market_id") || ((structural.contains("price") || structural.contains("cost")) && (structural.contains("item_data") || structural.contains("results")));
    let mentioned = message_mentions_market(value);
    if !route_is_market && !auction_post && !listing_shape && !mentioned {
        return None;
    }

    let kind = if auction_post {
        "auction_post"
    } else if listing_shape && !client_envelope {
        "listing_candidate"
    } else if route_is_market {
        "market_route"
    } else {
        "market_message"
    };
    Some(MarketObservation {
        direction: if client_envelope { "client" } else { "server_or_unknown" },
        kind,
        route,
        item_name: item_name_of(value),
        structural_fields: structural.into_iter().collect(),
        redacted_fields: sensitive.into_iter().collect(),
    })
}

pub fn observations_from_messages(messages: &[Value]) -> Vec<MarketObservation> {
    messages.iter().filter_map(observation).collect()
}

pub fn append_observations(path: &Path, messages: &[Value], flow: Flow, adapter: &str) -> io::Result<usize> {
    let observations = observations_from_messages(messages);
    if observations.is_empty() {
        return Ok(0);
    }
    let _guard = WRITE_LOCK.lock().map_err(|_| io::Error::other("market log lock poisoned"))?;
    let mut writer = observation_writer(path)?;
    let observed_at_unix_ms = unix_ms();
    for observation in &observations {
        let record = ObservationRecord {
            observed_at_unix_ms,
            record_type: "plaintext_market_structure",
            flow_tag: flow_tag(flow),
            adapter_tag: opaque_tag(adapter),
            observation,
        };
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(observations.len())
}

/// Append one-second summaries for port 443. TLS framing is a measured hint,
/// not inferred from the port; no payload bytes are copied or persisted.
pub fn append_port_443_windows(path: &Path, windows: &HashMap<Flow, Port443Summary>, adapter: &str) -> io::Result<usize> {
    if windows.is_empty() {
        return Ok(0);
    }
    let _guard = WRITE_LOCK.lock().map_err(|_| io::Error::other("market log lock poisoned"))?;
    let mut writer = observation_writer(path)?;
    let observed_at_unix_ms = unix_ms();
    let mut ordered: Vec<_> = windows.iter().collect();
    ordered.sort_by_key(|(flow, _)| **flow);
    for (flow, summary) in ordered {
        let record = Port443WindowRecord {
            observed_at_unix_ms,
            record_type: "port_443_flow_window",
            flow_tag: flow_tag(*flow),
            adapter_tag: opaque_tag(adapter),
            packet_count: summary.packet_count,
            payload_bytes: summary.payload_bytes,
            tls_like_packet_count: summary.tls_like_packet_count,
            payload_recorded: false,
        };
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(windows.len())
}

pub fn looks_like_tls_record(payload: &[u8]) -> bool {
    payload.len() >= 5 && matches!(payload[0], 0x14..=0x17) && payload[1] == 0x03 && payload[2] <= 0x04 && u16::from_be_bytes([payload[3], payload[4]]) <= 18_432
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{append_observations, looks_like_tls_record, observations_from_messages, rotate_if_needed};
    use crate::parser::Flow;

    #[test]
    fn auction_post_keeps_item_name_but_never_secret_values() {
        let message = json!({
            "account_id": "example-account",
            "checksum": "example-checksum",
            "fingerprint": "example-fingerprint",
            "identifier": "example-session",
            "item_data": {"a": 101, "b": 14, "sh": "example-hash"},
            "item_mask": "example-mask",
            "item_name": "Pillar of Niflheim"
        });
        let observations = observations_from_messages(&[message]);
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(observation.kind, "auction_post");
        assert_eq!(observation.direction, "client");
        assert_eq!(observation.item_name.as_deref(), Some("Pillar of Niflheim"));
        assert!(observation.redacted_fields.contains(&"identifier"));
        assert!(observation.redacted_fields.contains(&"checksum"));

        let serialized = serde_json::to_string(observation).unwrap();
        for secret in ["example-session", "example-checksum", "example-fingerprint", "example-mask"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn server_listing_shape_records_structure_only() {
        let message = json!({
            "results": [{"market_id": 42, "price": 123456, "item_data": {"a": 9}}],
            "status": 1
        });
        let observations = observations_from_messages(&[message]);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].kind, "listing_candidate");
        assert_eq!(observations[0].direction, "server_or_unknown");
        let serialized = serde_json::to_string(&observations[0]).unwrap();
        assert!(!serialized.contains("123456"));
        assert!(!serialized.contains("42"));
    }

    #[test]
    fn unrelated_game_messages_are_ignored() {
        let observations = observations_from_messages(&[json!({
            "currency_data": {"GSS": 123},
            "message": "saved"
        })]);
        assert!(observations.is_empty());
    }

    #[test]
    fn market_route_is_retained_without_query_values() {
        let observations = observations_from_messages(&[json!({
            "__route": "market/market_player_get_items_on_sale",
            "identifier": "example-session",
            "query": "Pillar"
        })]);
        assert_eq!(observations[0].route.as_deref(), Some("market/market_player_get_items_on_sale"));
        let serialized = serde_json::to_string(&observations[0]).unwrap();
        assert!(!serialized.contains("example-session"));
        assert!(!serialized.contains("Pillar"));
    }

    #[test]
    fn dynamic_route_segments_are_never_written_verbatim() {
        let observations = observations_from_messages(&[json!({
            "__route": "market/account/example-account/example-token",
            "identifier": "example-session"
        })]);
        let serialized = serde_json::to_string(&observations[0]).unwrap();
        assert_eq!(observations[0].route.as_deref(), Some("market/<redacted>"));
        assert!(!serialized.contains("example-account"));
        assert!(!serialized.contains("example-token"));
    }

    #[test]
    fn tls_is_identified_by_record_framing_not_by_port() {
        assert!(looks_like_tls_record(b"\x17\x03\x03\x00\x05abcde"));
        assert!(!looks_like_tls_record(b"market/plaintext"));
    }

    #[test]
    fn serialized_records_use_opaque_flow_tags() {
        let path = std::env::temp_dir().join(format!("hs-tracker-market-{}-flow.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        let flow: Flow = ("192.0.2.10".parse().unwrap(), 51_234, 443);
        let message = json!({
            "__route": "market/market_player_get_items_on_sale",
            "identifier": "example-session"
        });

        append_observations(&path, &[message], flow, "example-adapter").unwrap();
        let serialized = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(serialized.contains("\"flow_tag\""));
        assert!(!serialized.contains("192.0.2.10"));
        assert!(!serialized.contains("51234"));
        assert!(!serialized.contains("example-adapter"));
    }

    #[test]
    fn observation_log_rotates_at_its_bound() {
        let path = std::env::temp_dir().join(format!("hs-tracker-market-{}-rotate.jsonl", std::process::id()));
        let old = path.with_extension("old.jsonl");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&old);
        fs::write(&path, b"0123456789").unwrap();

        rotate_if_needed(&path, 10).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(&old).unwrap(), b"0123456789");

        let _ = fs::remove_file(&old);
    }
}
