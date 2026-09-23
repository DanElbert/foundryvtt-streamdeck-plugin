use crate::config::{Config, Indicator, OFFLINE_COLOR};
use crate::connection;
use crate::foundry::{self, ArtSource};
use crate::image::{ArtCache, ArtEntry, ArtKey, wrap_svg};
use crate::relay::Relay;

use openaction::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SheetSettings {
	pub actor_uuid: String,
	pub actor_name: String,
	pub art_source: ArtSource,
	pub show_title: bool,
}

impl Default for SheetSettings {
	fn default() -> Self {
		Self {
			actor_uuid: String::new(),
			actor_name: String::new(),
			art_source: ArtSource::Token,
			show_title: true,
		}
	}
}

#[derive(Default)]
pub struct SheetState {
	instances: RwLock<HashMap<InstanceId, SheetSettings>>,
	open: RwLock<HashMap<String, bool>>,
	art: ArtCache,
	inflight: Mutex<HashSet<ArtKey>>,
}

impl SheetState {
	pub async fn clear_art(&self) {
		self.art.clear().await;
	}

	pub async fn apply_snapshot(&self, uuids: &[Value]) -> bool {
		let next: HashMap<String, bool> = uuids
			.iter()
			.filter_map(Value::as_str)
			.map(|u| (u.to_string(), true))
			.collect();
		let mut open = self.open.write().await;
		let changed = open.iter().any(|(u, o)| *o != next.contains_key(u))
			|| next.keys().any(|u| open.get(u) != Some(&true));
		*open = next;
		changed
	}

	pub async fn apply_payload(&self, payload: &Value) -> bool {
		match payload.get("event").and_then(Value::as_str) {
			Some("sheet") => {
				let (Some(uuid), Some(is_open)) = (
					payload.get("uuid").and_then(Value::as_str),
					payload.get("open").and_then(Value::as_bool),
				) else {
					log::warn!("malformed sheet event: {payload}");
					return false;
				};
				self.open.write().await.insert(uuid.to_string(), is_open) != Some(is_open)
			}
			Some("snapshot") => match payload.get("open").and_then(Value::as_array) {
				Some(uuids) => self.apply_snapshot(uuids).await,
				None => {
					log::warn!("malformed snapshot event: {payload}");
					false
				}
			},
			other => {
				log::debug!("ignoring companion event {other:?}");
				false
			}
		}
	}
}

#[derive(Clone)]
pub struct Sheet {
	pub relay: Arc<Relay>,
	pub config: Arc<RwLock<Config>>,
	pub state: Arc<SheetState>,
}

impl Sheet {
	async fn art_key(&self, settings: &SheetSettings) -> ArtKey {
		ArtKey {
			uuid: settings.actor_uuid.clone(),
			source: settings.art_source,
			open_color: self.config.read().await.border_color.clone(),
		}
	}

	async fn paint(&self, instance: &Instance, settings: &SheetSettings) -> OpenActionResult<()> {
		if settings.actor_uuid.trim().is_empty() {
			instance.set_image(None::<String>, None).await?;
			return instance.set_title(Some("Pick actor"), None).await;
		}

		let online = self.relay.is_connected();
		let open = *self
			.state
			.open
			.read()
			.await
			.get(&settings.actor_uuid)
			.unwrap_or(&false);

		let key = self.art_key(settings).await;
		let entry = self.state.art.get(&key).await;
		let (indicator, open_color) = {
			let config = self.config.read().await;
			(config.indicator, config.border_color.clone())
		};

		let name = match entry.as_ref() {
			Some(e) if !e.name.is_empty() => e.name.clone(),
			_ if !settings.actor_name.is_empty() => settings.actor_name.clone(),
			_ => "?".to_string(),
		};

		let Some(entry) = entry else {
			instance.set_image(None::<String>, None).await?;
			return instance.set_title(Some(name), None).await;
		};

		let mut title = if settings.show_title {
			Some(name)
		} else {
			None
		};

		let image = match indicator {
			Indicator::Border => entry.variant(open, online).to_string(),
			Indicator::Svg => {
				let colour = if !online {
					Some(OFFLINE_COLOR)
				} else if open {
					Some(open_color.as_str())
				} else {
					None
				};
				wrap_svg(&entry.closed, colour)
			}
			Indicator::Title => {
				if let Some(t) = title.take() {
					title = Some(if open { format!("● {t}") } else { t });
				} else if open {
					title = Some("●".to_string());
				}
				entry.closed.clone()
			}
		};

		instance.set_image(Some(image), None).await?;
		instance.set_title(title, None).await
	}

	pub async fn repaint_visible(&self) {
		let mirror = self.state.instances.read().await.clone();
		for instance in visible_instances(Self::UUID).await {
			if let Some(settings) = mirror.get(&instance.instance_id) {
				let _ = self.paint(&instance, settings).await;
				self.refetch_if_missing(instance.instance_id.clone(), settings)
					.await;
			}
		}
	}

	async fn refetch_if_missing(&self, instance_id: InstanceId, settings: &SheetSettings) {
		if !self.relay.is_connected()
			|| self
				.state
				.art
				.get(&self.art_key(settings).await)
				.await
				.is_some()
		{
			return;
		}
		let this = self.clone();
		let settings = settings.clone();
		tokio::spawn(async move { this.ensure_art(instance_id, settings).await });
	}

	async fn ensure_art(&self, instance_id: InstanceId, settings: SheetSettings) {
		if settings.actor_uuid.trim().is_empty() {
			return;
		}
		let key = self.art_key(&settings).await;
		if self.state.art.get(&key).await.is_some() {
			return;
		}
		if !self.state.inflight.lock().await.insert(key.clone()) {
			return;
		}

		let (open_color, _) = {
			let config = self.config.read().await;
			(config.border_color.clone(), config.indicator)
		};
		let script = foundry::art_script(
			&settings.actor_uuid,
			settings.art_source,
			&open_color,
			OFFLINE_COLOR,
		);
		let result = self.relay.execute_js(script).await;
		self.state.inflight.lock().await.remove(&key);

		match result {
			Ok(value) => {
				if let Some(error) = value.get("error").and_then(Value::as_str) {
					log::warn!(
						"artwork for {} failed: {error} (src={})",
						settings.actor_uuid,
						value.get("src").and_then(Value::as_str).unwrap_or("?")
					);
				} else {
					let get = |k: &str| {
						value
							.get(k)
							.and_then(Value::as_str)
							.unwrap_or("")
							.to_string()
					};
					let entry = ArtEntry {
						closed: get("closed"),
						open: get("open"),
						offline: get("offline"),
						name: get("name"),
					};
					if entry.closed.is_empty() {
						log::warn!("artwork for {} returned no image", settings.actor_uuid);
					} else {
						log::debug!(
							"artwork for {} cached ({} bytes/variant)",
							settings.actor_uuid,
							entry.closed.len()
						);
						self.state.art.insert(key, entry).await;
					}
					if let Some(is_open) = value.get("isOpen").and_then(Value::as_bool) {
						self.state
							.open
							.write()
							.await
							.insert(settings.actor_uuid.clone(), is_open);
					}
				}
			}
			Err(error) => log::warn!("artwork for {} failed: {error}", settings.actor_uuid),
		}

		if let Some(instance) = get_instance(instance_id).await {
			let _ = self.paint(&instance, &settings).await;
		}
	}

	pub async fn push_connection_state(&self, instance: &Instance) {
		connection::push_state(&self.relay, &self.config, instance).await;
	}
}

#[async_trait]
impl Action for Sheet {
	const UUID: ActionUuid = "us.elbert.foundryvtt.sheet";
	type Settings = SheetSettings;

	async fn will_appear(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		self.state
			.instances
			.write()
			.await
			.insert(instance.instance_id.clone(), settings.clone());
		self.paint(instance, settings).await?;
		self.ensure_art(instance.instance_id.clone(), settings.clone())
			.await;
		Ok(())
	}

	async fn did_receive_settings(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		let changed = self
			.state
			.instances
			.write()
			.await
			.insert(instance.instance_id.clone(), settings.clone())
			.is_none_or(|old| old != *settings);
		self.paint(instance, settings).await?;
		if changed {
			self.ensure_art(instance.instance_id.clone(), settings.clone())
				.await;
		}
		Ok(())
	}

	async fn will_disappear(
		&self,
		instance: &Instance,
		_settings: &Self::Settings,
	) -> OpenActionResult<()> {
		self.state
			.instances
			.write()
			.await
			.remove(&instance.instance_id);
		Ok(())
	}

	async fn key_up(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
		if settings.actor_uuid.trim().is_empty() {
			return instance.show_alert().await;
		}

		match self
			.relay
			.execute_js(foundry::toggle_script(&settings.actor_uuid))
			.await
		{
			Ok(value) => {
				if let Some(error) = value.get("error").and_then(Value::as_str) {
					log::warn!("toggle {} failed: {error}", settings.actor_uuid);
					return instance.show_alert().await;
				}
				let open = value.get("open").and_then(Value::as_bool).unwrap_or(false);
				self.state
					.open
					.write()
					.await
					.insert(settings.actor_uuid.clone(), open);
				self.repaint_visible().await;
				Ok(())
			}
			Err(error) => {
				log::warn!("toggle {} failed: {error}", settings.actor_uuid);
				instance.show_alert().await
			}
		}
	}

	async fn property_inspector_did_appear(
		&self,
		instance: &Instance,
		_settings: &Self::Settings,
	) -> OpenActionResult<()> {
		self.push_connection_state(instance).await;
		Ok(())
	}

	async fn send_to_plugin(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
		payload: &Value,
	) -> OpenActionResult<()> {
		match payload.get("event").and_then(Value::as_str) {
			Some("getActors") => {
				let reply = match self.relay.search_actors().await {
					Ok(actors) => json!({
						"event": "actors",
						"status": "ok",
						"actors": actors.iter().map(|a| json!({"uuid": a.uuid, "name": a.name}))
							.collect::<Vec<_>>(),
					}),
					Err(error) => json!({
						"event": "actors",
						"status": "error",
						"message": error.to_string(),
					}),
				};
				let _ = instance.send_to_property_inspector(reply).await;
			}
			Some("refreshArt") => {
				let key = self.art_key(settings).await;
				self.state.art.remove(&key).await;
				self.ensure_art(instance.instance_id.clone(), settings.clone())
					.await;
			}
			Some("getConnection") => self.push_connection_state(instance).await,
			Some("setConnection") => {
				connection::apply_set_connection(&self.config, payload).await?
			}
			other => log::warn!("unknown sendToPlugin event: {other:?}"),
		}
		Ok(())
	}
}
