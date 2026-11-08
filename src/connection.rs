use crate::config::{Config, DEFAULT_BIND, Indicator};
use crate::server::Server;

use openaction::*;
use serde_json::{Value, json};
use tokio::sync::RwLock;

pub async fn push_state(server: &Server, config: &RwLock<Config>, instance: &Instance) {
	let config = config.read().await.clone();
	let _ = instance
		.send_to_property_inspector(json!({
			"event": "connection",
			"bind": config.bind,
			"port": config.port,
			"url": config.url(),
			"token": config.token,
			"hasToken": !config.token.trim().is_empty(),
			"listening": server.is_listening(),
			"listenError": server.listen_error().await,
			"connected": server.is_connected(),
			"companion": server.companion().await,
			"indicator": config.indicator,
			"borderColor": config.border_color,
		}))
		.await;
}

pub async fn apply_set_connection(
	config: &RwLock<Config>,
	payload: &Value,
) -> OpenActionResult<()> {
	let mut next = config.read().await.clone();
	if let Some(v) = payload.get("bind").and_then(Value::as_str) {
		let v = v.trim();
		next.bind = if v.is_empty() { DEFAULT_BIND.to_string() } else { v.to_string() };
	}
	if let Some(port) = payload.get("port").and_then(parse_port) {
		next.port = port;
	}
	if let Some(v) = payload.get("token").and_then(Value::as_str)
		&& !v.trim().is_empty()
	{
		next.token = v.trim().to_string();
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
	set_global_settings(&next).await?;
	get_global_settings().await
}

fn parse_port(value: &Value) -> Option<u16> {
	let port = match value {
		Value::Number(n) => n.as_u64()?,
		Value::String(s) => s.trim().parse::<u64>().ok()?,
		_ => return None,
	};
	u16::try_from(port).ok().filter(|p| *p != 0)
}
