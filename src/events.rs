use crate::condition::Condition;
use crate::foundry;
use crate::initiative::Initiative;
use crate::relay::RelayError;
use crate::sheet::Sheet;

use openaction::*;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
pub struct Actions {
	pub sheet: Sheet,
	pub condition: Condition,
	pub initiative: Initiative,
}

impl Actions {
	pub async fn repaint_all(&self) {
		self.sheet.repaint_visible().await;
		self.condition.repaint_visible().await;
		self.initiative.repaint_visible().await;
	}

	pub async fn clear_art(&self) {
		self.sheet.state.clear_art().await;
		self.condition.state.clear_art().await;
	}

	pub async fn push_connection_state_to_all(&self) {
		for instance in visible_instances(Sheet::UUID).await {
			self.sheet.push_connection_state(&instance).await;
		}
		for instance in visible_instances(Condition::UUID).await {
			self.condition.push_connection_state(&instance).await;
		}
		for instance in visible_instances(Initiative::UUID).await {
			self.initiative.push_connection_state(&instance).await;
		}
	}
}

pub async fn event_loop(actions: Actions) {
	let Actions {
		sheet,
		condition,
		initiative,
	} = &actions;
	let mut events = sheet.relay.events.subscribe();
	loop {
		match events.recv().await {
			Ok(payload) => match payload.get("event").and_then(Value::as_str) {
				Some("sheet" | "snapshot") => {
					if let Some(version) = payload.get("version").and_then(Value::as_str)
						&& sheet.relay.companion().await.as_deref() != Some(version)
					{
						log::info!(
							"companion module {} v{version} is active",
							foundry::COMPANION_ID
						);
						sheet.relay.set_companion(Some(version.to_string())).await;
						actions.push_connection_state_to_all().await;
					}
					if sheet.state.apply_payload(&payload).await {
						sheet.repaint_visible().await;
						initiative.repaint_visible().await;
					}
				}
				Some("selection") => {
					if condition.state.apply_selection(&payload).await {
						condition.repaint_visible().await;
						initiative.repaint_visible().await;
					}
				}
				Some("combat") => {
					if initiative
						.state
						.apply_combat(payload.get("combat").filter(|v| !v.is_null()))
						.await
					{
						initiative.repaint_visible().await;
					}
				}
				other => log::debug!("ignoring companion event {other:?}"),
			},
			Err(RecvError::Lagged(skipped)) => {
				log::warn!("dropped {skipped} companion event(s); resyncing");
				resync(actions.clone()).await;
			}
			Err(RecvError::Closed) => return,
		}
	}
}

pub async fn resync(actions: Actions) {
	let Actions {
		sheet,
		condition,
		initiative,
	} = &actions;
	let value = match foundry::sync(&sheet.relay).await {
		Ok(value) => value,
		Err(RelayError::Timeout) => {
			log::warn!(
				"companion module '{}' did not answer; it is not active in this world. Buttons \
				 will not track Foundry, and presses will alert until it is enabled",
				foundry::COMPANION_ID
			);
			Value::Null
		}
		Err(RelayError::Remote(message)) if message.contains("Unknown message type") => {
			log::error!(
				"relay rejected the '{}' request type ({message}); it needs the \
				 foundryvtt-rest-api-relay fork patch",
				foundry::REQUEST_TYPE
			);
			Value::Null
		}
		Err(error) => {
			log::warn!("state sync failed: {error}");
			return;
		}
	};

	let companion = value
		.get("version")
		.and_then(Value::as_str)
		.map(str::to_string);
	if let Some(version) = &companion {
		log::info!("companion module {} v{version}", foundry::COMPANION_ID);
	}
	sheet.relay.set_companion(companion).await;

	if let Some(uuids) = value.get("open").and_then(Value::as_array)
		&& sheet.state.apply_snapshot(uuids).await
	{
		sheet.repaint_visible().await;
	}

	let selection = value.get("selection").filter(|v| !v.is_null());
	let changed = match selection {
		Some(selection) => condition.state.apply_selection(selection).await,
		None => condition.state.reset_selection().await,
	};
	if changed {
		condition.repaint_visible().await;
	}

	if initiative
		.state
		.apply_combat(value.get("combat").filter(|v| !v.is_null()))
		.await || changed
	{
		initiative.repaint_visible().await;
	}

	actions.push_connection_state_to_all().await;
}
