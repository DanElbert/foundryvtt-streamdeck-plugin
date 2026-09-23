use crate::condition::ConditionState;
use crate::connection;
use crate::foundry::{self, ArtSource};
use crate::sheet::{Sheet, SheetSettings};

use base64::Engine;
use openaction::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

const SWORDS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 72 72" width="144" height="144"><rect width="72" height="72" rx="14" fill="#1b1d24"/><g opacity="OPACITY" fill="none" stroke="#ff6400" stroke-width="4" stroke-linecap="round"><path d="M17 17 L45 45"/><path d="M55 17 L27 45"/><path d="M38 52 L52 38"/><path d="M20 38 L34 52"/><path d="M47 47 L56 56"/><path d="M25 47 L16 56"/></g></svg>"##;

fn swords(bright: bool) -> String {
	let svg = SWORDS.replace("OPACITY", if bright { "1" } else { "0.35" });
	format!(
		"data:image/svg+xml;base64,{}",
		base64::engine::general_purpose::STANDARD.encode(svg)
	)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InitiativeSettings {
	pub show_title: bool,
}

impl Default for InitiativeSettings {
	fn default() -> Self {
		Self { show_title: true }
	}
}

#[derive(Clone, PartialEq, Eq)]
pub struct Combatant {
	pub name: String,
	pub actor_uuid: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CombatInfo {
	pub round: u64,
	pub started: bool,
	pub combatant: Option<Combatant>,
}

impl CombatInfo {
	fn from_value(value: &Value) -> Option<Self> {
		let object = value.as_object()?;
		let combatant = object
			.get("combatant")
			.and_then(Value::as_object)
			.map(|c| Combatant {
				name: c
					.get("name")
					.and_then(Value::as_str)
					.unwrap_or("?")
					.to_string(),
				actor_uuid: c
					.get("actorUuid")
					.and_then(Value::as_str)
					.filter(|u| !u.is_empty())
					.map(str::to_string),
			});
		Some(Self {
			round: object.get("round").and_then(Value::as_u64).unwrap_or(0),
			started: object
				.get("started")
				.and_then(Value::as_bool)
				.unwrap_or(false),
			combatant,
		})
	}
}

#[derive(Default)]
pub struct InitiativeState {
	instances: RwLock<HashMap<InstanceId, InitiativeSettings>>,
	combat: RwLock<Option<CombatInfo>>,
	art_ready: Notify,
}

impl InitiativeState {
	pub async fn apply_combat(&self, value: Option<&Value>) -> bool {
		let next = value.and_then(CombatInfo::from_value);
		let mut current = self.combat.write().await;
		if *current == next {
			return false;
		}
		*current = next;
		true
	}
}

#[derive(Clone)]
pub struct Initiative {
	pub sheet: Sheet,
	pub condition: Arc<ConditionState>,
	pub state: Arc<InitiativeState>,
}

fn art_settings(uuid: &str) -> SheetSettings {
	SheetSettings {
		actor_uuid: uuid.to_string(),
		actor_name: String::new(),
		art_source: ArtSource::Token,
		show_title: true,
	}
}

impl Initiative {
	async fn paint(
		&self,
		instance: &Instance,
		settings: &InitiativeSettings,
	) -> OpenActionResult<()> {
		let online = self.sheet.relay.is_connected();
		let combat = self.state.combat.read().await.clone();

		let Some(combat) = combat else {
			let count = self.condition.selection_count().await;
			let ready = online && count > 0;
			instance.set_image(Some(swords(ready)), None).await?;
			let title = ready.then(|| format!("Start ({count})"));
			return instance.set_title(title, None).await;
		};

		let Some(Combatant {
			name,
			actor_uuid: Some(uuid),
		}) = combat.combatant
		else {
			instance.set_image(Some(swords(online)), None).await?;
			return instance.set_title(Some("Empty"), None).await;
		};

		let round = combat.started.then(|| format!("R{}", combat.round));
		let title = match (settings.show_title, round) {
			(true, Some(r)) => Some(format!("{name}\n{r}")),
			(true, None) => Some(name),
			(false, round) => round,
		};

		let art = art_settings(&uuid);
		match self.sheet.cached_art(&art).await {
			Some(entry) => {
				let open = self.sheet.is_open(&uuid).await;
				let (image, title) = self.sheet.styled(&entry, open, online, title).await;
				instance.set_image(Some(image), None).await?;
				instance.set_title(title, None).await
			}
			None => {
				instance.set_image(None::<String>, None).await?;
				instance.set_title(title, None).await?;
				if online {
					let sheet = self.sheet.clone();
					let state = self.state.clone();
					tokio::spawn(async move {
						if sheet.fetch_art(&art).await && sheet.cached_art(&art).await.is_some() {
							state.art_ready.notify_one();
						}
					});
				}
				Ok(())
			}
		}
	}

	pub async fn repaint_visible(&self) {
		let mirror = self.state.instances.read().await.clone();
		for instance in visible_instances(Self::UUID).await {
			if let Some(settings) = mirror.get(&instance.instance_id) {
				let _ = self.paint(&instance, settings).await;
			}
		}
	}

	pub async fn push_connection_state(&self, instance: &Instance) {
		connection::push_state(&self.sheet.relay, &self.sheet.config, instance).await;
	}

	pub async fn art_loop(self) {
		loop {
			self.state.art_ready.notified().await;
			self.repaint_visible().await;
		}
	}
}

#[async_trait]
impl Action for Initiative {
	const UUID: ActionUuid = "us.elbert.foundryvtt.initiative";
	type Settings = InitiativeSettings;

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
		self.paint(instance, settings).await
	}

	async fn did_receive_settings(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		self.state
			.instances
			.write()
			.await
			.insert(instance.instance_id.clone(), settings.clone());
		self.paint(instance, settings).await
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

	async fn key_up(
		&self,
		instance: &Instance,
		_settings: &Self::Settings,
	) -> OpenActionResult<()> {
		let combat = self.state.combat.read().await.clone();

		if combat.is_some() && !foundry::companion_ready(&self.sheet.relay).await {
			return instance.show_alert().await;
		}

		if let Some(combat) = combat {
			let Some(uuid) = combat.combatant.and_then(|c| c.actor_uuid) else {
				return instance.show_alert().await;
			};
			return match self.sheet.toggle(&uuid).await {
				Ok(()) => {
					self.repaint_visible().await;
					Ok(())
				}
				Err(error) => {
					log::warn!("toggle combatant sheet {uuid} failed: {error}");
					instance.show_alert().await
				}
			};
		}

		if self.condition.selection_count().await == 0 {
			return instance.show_alert().await;
		}

		match foundry::start_combat(&self.sheet.relay).await {
			Ok(value) => match value.get("error").and_then(Value::as_str) {
				Some(error) => {
					log::warn!("start combat failed: {error}");
					instance.show_alert().await
				}
				None => Ok(()),
			},
			Err(error) => {
				log::warn!("start combat failed: {error}");
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
		_settings: &Self::Settings,
		payload: &Value,
	) -> OpenActionResult<()> {
		match payload.get("event").and_then(Value::as_str) {
			Some("getConnection") => self.push_connection_state(instance).await,
			Some("setConnection") => {
				connection::apply_set_connection(&self.sheet.config, payload).await?
			}
			other => log::warn!("unknown sendToPlugin event: {other:?}"),
		}
		Ok(())
	}
}
