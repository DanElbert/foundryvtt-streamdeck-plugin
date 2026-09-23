use crate::config::{Config, OFFLINE_COLOR};
use crate::connection;
use crate::foundry;
use crate::image::{ConditionArt, ConditionKey, TtlCache};
use crate::relay::Relay;

use openaction::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConditionSettings {
	pub status_id: String,
	pub status_name: String,
	pub show_title: bool,
}

impl Default for ConditionSettings {
	fn default() -> Self {
		Self {
			status_id: String::new(),
			status_name: String::new(),
			show_title: true,
		}
	}
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Selection {
	pub count: u32,
	pub statuses: HashMap<String, u32>,
}

impl Selection {
	fn from_value(value: &Value) -> Option<Self> {
		let count = u32::try_from(value.get("count")?.as_u64()?).ok()?;
		let statuses = value
			.get("statuses")?
			.as_object()?
			.iter()
			.filter_map(|(id, n)| Some((id.clone(), u32::try_from(n.as_u64()?).ok()?)))
			.collect();
		Some(Self { count, statuses })
	}
}

#[derive(Clone, Copy)]
enum Variant {
	None,
	Off,
	On,
	Mixed,
	Offline,
}

impl Variant {
	fn pick(online: bool, selection: &Selection, id: &str) -> Self {
		if !online {
			return Self::Offline;
		}
		if selection.count == 0 {
			return Self::None;
		}
		match selection.statuses.get(id).copied().unwrap_or(0) {
			0 => Self::Off,
			n if n >= selection.count => Self::On,
			_ => Self::Mixed,
		}
	}

	fn image(self, art: &ConditionArt) -> &str {
		match self {
			Self::None => &art.none,
			Self::Off => &art.off,
			Self::On => &art.on,
			Self::Mixed => &art.mixed,
			Self::Offline => &art.offline,
		}
	}
}

#[derive(Default)]
pub struct ConditionState {
	instances: RwLock<HashMap<InstanceId, ConditionSettings>>,
	selection: RwLock<Selection>,
	art: TtlCache<ConditionKey, ConditionArt>,
	inflight: Mutex<HashSet<ConditionKey>>,
}

impl ConditionState {
	pub async fn clear_art(&self) {
		self.art.clear().await;
	}

	pub async fn reset_selection(&self) -> bool {
		let mut current = self.selection.write().await;
		let changed = *current != Selection::default();
		*current = Selection::default();
		changed
	}

	pub async fn apply_selection(&self, value: &Value) -> bool {
		let Some(next) = Selection::from_value(value) else {
			log::warn!("malformed selection event: {value}");
			return false;
		};
		let mut current = self.selection.write().await;
		if *current == next {
			return false;
		}
		*current = next;
		true
	}
}

#[derive(Clone)]
pub struct Condition {
	pub relay: Arc<Relay>,
	pub config: Arc<RwLock<Config>>,
	pub state: Arc<ConditionState>,
}

impl Condition {
	async fn art_key(&self, settings: &ConditionSettings) -> ConditionKey {
		ConditionKey {
			id: settings.status_id.clone(),
			accent: self.config.read().await.border_color.clone(),
		}
	}

	async fn paint(
		&self,
		instance: &Instance,
		settings: &ConditionSettings,
	) -> OpenActionResult<()> {
		if settings.status_id.trim().is_empty() {
			instance.set_image(None::<String>, None).await?;
			return instance.set_title(Some("Pick condition"), None).await;
		}

		let entry = self.state.art.get(&self.art_key(settings).await).await;
		let name = match entry.as_ref() {
			Some(e) if !e.name.is_empty() => e.name.clone(),
			_ if !settings.status_name.is_empty() => settings.status_name.clone(),
			_ => settings.status_id.clone(),
		};

		let Some(entry) = entry else {
			instance.set_image(None::<String>, None).await?;
			return instance.set_title(Some(name), None).await;
		};

		let variant = {
			let selection = self.state.selection.read().await;
			Variant::pick(self.relay.is_connected(), &selection, &settings.status_id)
		};
		let title = settings.show_title.then_some(name);
		instance
			.set_image(Some(variant.image(&entry).to_string()), None)
			.await?;
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

	async fn refetch_if_missing(&self, instance_id: InstanceId, settings: &ConditionSettings) {
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

	async fn ensure_art(&self, instance_id: InstanceId, settings: ConditionSettings) {
		if settings.status_id.trim().is_empty() {
			return;
		}
		let key = self.art_key(&settings).await;
		if self.state.art.get(&key).await.is_some() {
			return;
		}
		if !self.state.inflight.lock().await.insert(key.clone()) {
			return;
		}

		let script = foundry::condition_art_script(&key.id, &key.accent, OFFLINE_COLOR);
		let result = self.relay.execute_js(script).await;
		self.state.inflight.lock().await.remove(&key);

		match result {
			Ok(value) => {
				if let Some(error) = value.get("error").and_then(Value::as_str) {
					log::warn!("condition art for {} failed: {error}", key.id);
				} else {
					let get = |k: &str| {
						value
							.get(k)
							.and_then(Value::as_str)
							.unwrap_or("")
							.to_string()
					};
					let art = ConditionArt {
						name: get("name"),
						off: get("off"),
						on: get("on"),
						mixed: get("mixed"),
						none: get("none"),
						offline: get("offline"),
					};
					if art.on.is_empty() {
						log::warn!("condition art for {} returned no image", key.id);
					} else {
						self.state.art.insert(key.clone(), art).await;
					}
				}
			}
			Err(error) => log::warn!("condition art for {} failed: {error}", key.id),
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
impl Action for Condition {
	const UUID: ActionUuid = "us.elbert.foundryvtt.condition";
	type Settings = ConditionSettings;

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
		let id = settings.status_id.trim();
		if id.is_empty() || self.state.selection.read().await.count == 0 {
			return instance.show_alert().await;
		}

		match self
			.relay
			.execute_js(foundry::toggle_condition_script(id))
			.await
		{
			Ok(value) => match value.get("error").and_then(Value::as_str) {
				Some(error) => {
					log::warn!("toggle condition {id} failed: {error}");
					instance.show_alert().await
				}
				None => Ok(()),
			},
			Err(error) => {
				log::warn!("toggle condition {id} failed: {error}");
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
			Some("getConditions") => {
				let reply = match self.relay.execute_js(foundry::conditions_script()).await {
					Ok(value) => {
						match (value.get("error").and_then(Value::as_str), value.as_array()) {
							(Some(error), _) => json!({
								"event": "conditions", "status": "error", "message": error,
							}),
							(None, Some(list)) => json!({
								"event": "conditions", "status": "ok", "conditions": list,
							}),
							(None, None) => json!({
								"event": "conditions", "status": "error", "message": "malformed reply",
							}),
						}
					}
					Err(error) => json!({
						"event": "conditions", "status": "error", "message": error.to_string(),
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
