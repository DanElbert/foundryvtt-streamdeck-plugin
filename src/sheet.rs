use crate::config::{Config, Indicator, OFFLINE_COLOR};
use crate::foundry::{self, ArtSource};
use crate::image::{ArtCache, ArtEntry, ArtKey, wrap_svg};
use crate::relay::Relay;

use openaction::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock};

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
	pub poll_wake: Notify,
	poll_disabled: AtomicBool,
}

impl SheetState {
	pub async fn clear_art(&self) {
		self.art.clear().await;
	}

	pub fn set_poll_disabled(&self, disabled: bool) {
		self.poll_disabled.store(disabled, Ordering::Relaxed);
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
			}
		}
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
					let entry =
						ArtEntry::new(get("closed"), get("open"), get("offline"), get("name"));
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
		let config = self.config.read().await.clone();
		let _ = instance
			.send_to_property_inspector(json!({
				"event": "connection",
				"relayUrl": config.relay_url,
				"clientId": config.client_id,
				"resolvedClientId": self.relay.resolved_client_id().await,
				"hasApiKey": !config.api_key.trim().is_empty(),
				"pollMs": config.poll_ms,
				"indicator": config.indicator,
				"borderColor": config.border_color,
				"connected": self.relay.is_connected(),
				"pollDisabled": self.state.poll_disabled.load(Ordering::Relaxed),
			}))
			.await;
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
		self.state.poll_wake.notify_waiters();
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
			self.state.poll_wake.notify_waiters();
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
				let mut next = self.config.read().await.clone();
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
				if let Some(v) = payload.get("pollMs").and_then(Value::as_u64) {
					next.poll_ms = v;
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
				set_global_settings(&next).await?;
			}
			other => log::warn!("unknown sendToPlugin event: {other:?}"),
		}
		Ok(())
	}
}

pub async fn poll_loop(sheet: Sheet) {
	loop {
		let (poll_ms, _) = {
			let config = sheet.config.read().await;
			(config.poll_ms, config.indicator)
		};

		if poll_ms == 0 || sheet.state.poll_disabled.load(Ordering::Relaxed) {
			sheet.state.poll_wake.notified().await;
			continue;
		}

		let mirror = sheet.state.instances.read().await.clone();
		let mut uuids: Vec<String> = visible_instances(Sheet::UUID)
			.await
			.iter()
			.filter_map(|i| mirror.get(&i.instance_id))
			.map(|s| s.actor_uuid.trim().to_string())
			.filter(|u| !u.is_empty())
			.collect();
		uuids.sort();
		uuids.dedup();

		if uuids.is_empty() {
			sheet.state.poll_wake.notified().await;
			continue;
		}

		if sheet.relay.is_connected() {
			match sheet.relay.execute_js(foundry::poll_script(&uuids)).await {
				Ok(value) => {
					let mut changed = false;
					if let Some(map) = value.as_object() {
						let mut open = sheet.state.open.write().await;
						for (uuid, state) in map {
							if let Some(is_open) = state.as_bool()
								&& open.insert(uuid.clone(), is_open) != Some(is_open)
							{
								changed = true;
							}
						}
					}
					if changed {
						sheet.repaint_visible().await;
					}
				}
				Err(error) => log::debug!("poll failed: {error}"),
			}
		}

		tokio::time::sleep(Duration::from_millis(poll_ms)).await;
	}
}

pub async fn push_connection_state_to_all(sheet: &Sheet) {
	for instance in visible_instances(Sheet::UUID).await {
		sheet.push_connection_state(&instance).await;
	}
}

pub async fn probe_notify_setting(sheet: Sheet) {
	match sheet.relay.execute_js(foundry::notify_probe_script()).await {
		Ok(value) => match value.get("notify").and_then(Value::as_bool) {
			Some(true) => {
				log::error!(
					"'Notify on Execute JS' is ENABLED in the Foundry REST API module. \
					 Polling is disabled for this session to avoid whispering the GM every \
					 interval. Turn that setting off and reconnect to restore live state sync."
				);
				sheet.state.set_poll_disabled(true);
			}
			_ => {
				sheet.state.set_poll_disabled(false);
				sheet.state.poll_wake.notify_waiters();
			}
		},
		Err(error) => log::warn!("could not read notifyOnExecuteJs: {error}"),
	}
}
