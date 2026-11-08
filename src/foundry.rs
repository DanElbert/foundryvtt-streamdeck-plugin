use crate::server::{REQUEST_TIMEOUT, Server, ServerError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const COMPANION_ID: &str = "foundryvtt-streamdeck";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtSource {
	Token,
	Portrait,
	Preferred,
}

pub async fn sync(server: &Server) -> Result<Value, ServerError> {
	server.call("snapshot", json!([]), REQUEST_TIMEOUT).await
}

pub async fn toggle_sheet(server: &Server, uuid: &str) -> Result<Value, ServerError> {
	server
		.call("toggleSheet", json!([uuid]), REQUEST_TIMEOUT)
		.await
}

pub async fn actor_art(
	server: &Server,
	uuid: &str,
	source: ArtSource,
	accent: &str,
	offline: &str,
) -> Result<Value, ServerError> {
	server
		.call(
			"actorArt",
			json!([uuid, { "source": source, "accent": accent, "offline": offline }]),
			REQUEST_TIMEOUT,
		)
		.await
}

pub async fn conditions(server: &Server) -> Result<Value, ServerError> {
	server.call("conditions", json!([]), REQUEST_TIMEOUT).await
}

pub async fn toggle_condition(server: &Server, id: &str) -> Result<Value, ServerError> {
	server
		.call("toggleCondition", json!([id]), REQUEST_TIMEOUT)
		.await
}

pub async fn condition_art(
	server: &Server,
	id: &str,
	accent: &str,
	offline: &str,
) -> Result<Value, ServerError> {
	server
		.call(
			"conditionArt",
			json!([id, { "accent": accent, "offline": offline }]),
			REQUEST_TIMEOUT,
		)
		.await
}

pub async fn start_combat(server: &Server) -> Result<Value, ServerError> {
	server.call("startCombat", json!([]), REQUEST_TIMEOUT).await
}

pub async fn actors(server: &Server) -> Result<Value, ServerError> {
	server.call("actors", json!([]), REQUEST_TIMEOUT).await
}

pub async fn macros(server: &Server) -> Result<Value, ServerError> {
	server.call("macros", json!([]), REQUEST_TIMEOUT).await
}

pub async fn execute_macro(server: &Server, uuid: &str) -> Result<Value, ServerError> {
	server
		.call("executeMacro", json!([uuid]), REQUEST_TIMEOUT)
		.await
}

pub async fn macro_art(server: &Server, uuid: &str, offline: &str) -> Result<Value, ServerError> {
	server
		.call(
			"macroArt",
			json!([uuid, { "offline": offline }]),
			REQUEST_TIMEOUT,
		)
		.await
}

pub fn list_reply(event: &str, key: &str, result: Result<Value, ServerError>) -> Value {
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
