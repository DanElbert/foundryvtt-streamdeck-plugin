use crate::config::{Config, OFFLINE_COLOR};
use crate::connection;
use crate::foundry;
use crate::image::{MacroArt, TtlCache};
use crate::relay::Relay;

use openaction::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MacroSettings {
	pub macro_uuid: String,
	pub macro_name: String,
	pub show_title: bool,
}

impl Default for MacroSettings {
	fn default() -> Self {
		Self {
			macro_uuid: String::new(),
			macro_name: String::new(),
			show_title: true,
		}
	}
}

#[derive(Default)]
pub struct MacroState {
	instances: RwLock<HashMap<InstanceId, MacroSettings>>,
	art: TtlCache<String, MacroArt>,
	inflight: Mutex<HashSet<String>>,
}

impl MacroState {
	pub async fn clear_art(&self) {
		self.art.clear().await;
	}
}

#[derive(Clone)]
pub struct Macro {
	pub relay: Arc<Relay>,
	pub config: Arc<RwLock<Config>>,
	pub state: Arc<MacroState>,
}

impl Macro {
	async fn paint(&self, instance: &Instance, settings: &MacroSettings) -> OpenActionResult<()> {
		let uuid = settings.macro_uuid.trim();
		if uuid.is_empty() {
			instance.set_image(None::<String>, None).await?;
			return instance.set_title(Some("Pick macro"), None).await;
		}

		let entry = self.state.art.get(&uuid.to_string()).await;
		let name = match entry.as_ref() {
			Some(e) if !e.name.is_empty() => e.name.clone(),
			_ if !settings.macro_name.is_empty() => settings.macro_name.clone(),
			_ => "?".to_string(),
		};

		let Some(entry) = entry else {
			instance.set_image(None::<String>, None).await?;
			return instance.set_title(Some(name), None).await;
		};

		let image = if self.relay.is_connected() {
			entry.ready
		} else {
			entry.offline
		};
		instance.set_image(Some(image), None).await?;
		instance
			.set_title(settings.show_title.then_some(name), None)
			.await
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

	async fn refetch_if_missing(&self, instance_id: InstanceId, settings: &MacroSettings) {
		if !self.relay.is_connected()
			|| self
				.state
				.art
				.get(&settings.macro_uuid.trim().to_string())
				.await
				.is_some()
		{
			return;
		}
		let this = self.clone();
		let settings = settings.clone();
		tokio::spawn(async move { this.ensure_art(instance_id, settings).await });
	}

	async fn ensure_art(&self, instance_id: InstanceId, settings: MacroSettings) {
		let uuid = settings.macro_uuid.trim().to_string();
		if uuid.is_empty() || self.state.art.get(&uuid).await.is_some() {
			return;
		}
		if !self.state.inflight.lock().await.insert(uuid.clone()) {
			return;
		}

		let result = foundry::macro_art(&self.relay, &uuid, OFFLINE_COLOR).await;
		self.state.inflight.lock().await.remove(&uuid);

		match result {
			Ok(value) => {
				if let Some(error) = value.get("error").and_then(Value::as_str) {
					log::warn!(
						"macro art for {uuid} failed: {error} (src={})",
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
					let art = MacroArt {
						name: get("name"),
						ready: get("ready"),
						offline: get("offline"),
					};
					if art.ready.is_empty() {
						log::warn!("macro art for {uuid} returned no image");
					} else {
						self.state.art.insert(uuid.clone(), art).await;
					}
				}
			}
			Err(error) => log::warn!("macro art for {uuid} failed: {error}"),
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
impl Action for Macro {
	const UUID: ActionUuid = "us.elbert.foundryvtt.macro";
	type Settings = MacroSettings;

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
		let uuid = settings.macro_uuid.trim();
		if uuid.is_empty() || !foundry::companion_ready(&self.relay).await {
			return instance.show_alert().await;
		}

		match foundry::execute_macro(&self.relay, uuid).await {
			Ok(value) => match value.get("error").and_then(Value::as_str) {
				Some(error) => {
					log::warn!("execute macro {uuid} failed: {error}");
					instance.show_alert().await
				}
				None => instance.show_ok().await,
			},
			Err(error) => {
				log::warn!("execute macro {uuid} failed: {error}");
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
			Some("getMacros") => {
				let reply =
					foundry::list_reply("macros", "macros", foundry::macros(&self.relay).await);
				let _ = instance.send_to_property_inspector(reply).await;
			}
			Some("refreshArt") => {
				self.state
					.art
					.remove(&settings.macro_uuid.trim().to_string())
					.await;
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
