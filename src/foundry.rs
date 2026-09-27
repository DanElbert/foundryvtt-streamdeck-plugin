use crate::relay::{REQUEST_TIMEOUT, Relay, RelayError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

pub const COMPANION_ID: &str = "foundryvtt-streamdeck";
pub const EVENT_HOOK: &str = "foundryvtt-streamdeck.event";
pub const REQUEST_TYPE: &str = "streamdeck";
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtSource {
	Token,
	Portrait,
	Preferred,
}

pub async fn sync(relay: &Relay) -> Result<Value, RelayError> {
	relay.call("snapshot", json!([]), SYNC_TIMEOUT).await
}

pub async fn companion_ready(relay: &Relay) -> bool {
	if relay.companion().await.is_some() {
		return true;
	}
	match sync(relay).await {
		Ok(value) => {
			let version = value
				.get("version")
				.and_then(Value::as_str)
				.map(str::to_string);
			let ready = version.is_some();
			relay.set_companion(version).await;
			ready
		}
		Err(_) => false,
	}
}

pub async fn toggle_sheet(relay: &Relay, uuid: &str) -> Result<Value, RelayError> {
	relay
		.call("toggleSheet", json!([uuid]), REQUEST_TIMEOUT)
		.await
}

pub async fn actor_art(
	relay: &Relay,
	uuid: &str,
	source: ArtSource,
	accent: &str,
	offline: &str,
) -> Result<Value, RelayError> {
	relay
		.call(
			"actorArt",
			json!([uuid, { "source": source, "accent": accent, "offline": offline }]),
			REQUEST_TIMEOUT,
		)
		.await
}

pub async fn conditions(relay: &Relay) -> Result<Value, RelayError> {
	relay.call("conditions", json!([]), REQUEST_TIMEOUT).await
}

pub async fn toggle_condition(relay: &Relay, id: &str) -> Result<Value, RelayError> {
	relay
		.call("toggleCondition", json!([id]), REQUEST_TIMEOUT)
		.await
}

pub async fn condition_art(
	relay: &Relay,
	id: &str,
	accent: &str,
	offline: &str,
) -> Result<Value, RelayError> {
	relay
		.call(
			"conditionArt",
			json!([id, { "accent": accent, "offline": offline }]),
			REQUEST_TIMEOUT,
		)
		.await
}

pub async fn start_combat(relay: &Relay) -> Result<Value, RelayError> {
	relay.call("startCombat", json!([]), REQUEST_TIMEOUT).await
}

pub async fn actors(relay: &Relay) -> Result<Value, RelayError> {
	relay.call("actors", json!([]), REQUEST_TIMEOUT).await
}

pub async fn macros(relay: &Relay) -> Result<Value, RelayError> {
	relay.call("macros", json!([]), REQUEST_TIMEOUT).await
}

pub async fn execute_macro(relay: &Relay, uuid: &str) -> Result<Value, RelayError> {
	relay
		.call("executeMacro", json!([uuid]), REQUEST_TIMEOUT)
		.await
}

pub async fn macro_art(relay: &Relay, uuid: &str, offline: &str) -> Result<Value, RelayError> {
	relay
		.call(
			"macroArt",
			json!([uuid, { "offline": offline }]),
			REQUEST_TIMEOUT,
		)
		.await
}

pub fn list_reply(event: &str, key: &str, result: Result<Value, RelayError>) -> Value {
	match result {
		Ok(value) => match (value.get("error").and_then(Value::as_str), value.as_array()) {
			(Some(error), _) => json!({ "event": event, "status": "error", "message": error }),
			(None, Some(list)) => json!({ "event": event, "status": "ok", key: list }),
			(None, None) => {
				json!({ "event": event, "status": "error", "message": "malformed reply" })
			}
		},
		Err(error) => json!({ "event": event, "status": "error", "message": error.to_string() }),
	}
}
