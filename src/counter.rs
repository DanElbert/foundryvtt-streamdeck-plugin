use openaction::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CounterSettings {
	step: isize,
	value: isize,
	label: String,
}

impl Default for CounterSettings {
	fn default() -> Self {
		Self {
			step: 1,
			value: 0,
			label: String::new(),
		}
	}
}

fn render_title(settings: &CounterSettings) -> String {
	let label = settings.label.trim();
	if label.is_empty() {
		settings.value.to_string()
	} else {
		format!("{}\n{}", label, settings.value)
	}
}

async fn refresh_title(instance: &Instance, settings: &CounterSettings) -> OpenActionResult<()> {
	instance.set_title(Some(render_title(settings)), None).await
}

pub struct Counter;

#[async_trait]
impl Action for Counter {
	const UUID: ActionUuid = "us.elbert.foundryvtt.counter";
	type Settings = CounterSettings;

	async fn will_appear(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		refresh_title(instance, settings).await
	}

	async fn did_receive_settings(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		refresh_title(instance, settings).await
	}

	async fn key_up(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
		let mut next = settings.clone();
		next.value += next.step;
		instance.set_settings(&next).await?;
		refresh_title(instance, &next).await
	}
}
