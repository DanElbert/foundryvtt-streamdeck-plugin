use crate::config::{Config, Indicator};
use crate::relay::Relay;

use openaction::*;
use serde_json::{Value, json};
use tokio::sync::RwLock;

pub async fn push_state(relay: &Relay, config: &RwLock<Config>, instance: &Instance) {
	let config = config.read().await.clone();
	let _ = instance
		.send_to_property_inspector(json!({
			"event": "connection",
			"relayUrl": config.relay_url,
			"clientId": config.client_id,
			"resolvedClientId": relay.resolved_client_id().await,
			"hasApiKey": !config.api_key.trim().is_empty(),
			"indicator": config.indicator,
			"borderColor": config.border_color,
			"connected": relay.is_connected(),
			"companion": relay.companion().await,
		}))
		.await;
}

pub async fn apply_set_connection(
	config: &RwLock<Config>,
	payload: &Value,
) -> OpenActionResult<()> {
	let mut next = config.read().await.clone();
	if let Some(v) = payload.get("relayUrl").and_then(Value::as_str) {
		next.relay_url = v.trim().to_string();
	}
	if let Some(v) = payload.get("clientId").and_then(Value::as_str) {
		next.client_id = v.trim().to_string();
	}
	if let Some(v) = payload.get("apiKey").and_then(Value::as_str)
		&& !v.is_empty()
	{
		next.api_key = v.to_string();
	}
	if let Some(v) = payload.get("indicator").and_then(Value::as_str) {
		next.indicator = match v {
			"svg" => Indicator::Svg,
			"title" => Indicator::Title,
			_ => Indicator::Border,
		};
	}
	if let Some(v) = payload.get("borderColor").and_then(Value::as_str) {
		next.border_color = v.trim().to_string();
	}
	// Only send. The host echoes didReceiveGlobalSettings, which is the single
	// place config is applied -- and which then pushes fresh state to the PI.
	// Pushing here would repaint the inspector from the pre-write config.
	set_global_settings(&next).await
}
