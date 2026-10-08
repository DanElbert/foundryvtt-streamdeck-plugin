use openaction::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;

const LONG_PRESS: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PressAction {
	Step,
	Reset,
	Nothing,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CounterSettings {
	value: isize,
	label: String,
	start: isize,
	short_action: PressAction,
	step: isize,
	long_action: PressAction,
	long_step: isize,
}

impl Default for CounterSettings {
	fn default() -> Self {
		Self {
			value: 0,
			label: String::new(),
			start: 0,
			short_action: PressAction::Step,
			step: 1,
			long_action: PressAction::Reset,
			long_step: 1,
		}
	}
}

fn apply(settings: &CounterSettings, action: PressAction, step: isize) -> CounterSettings {
	let mut next = settings.clone();
	match action {
		PressAction::Step => next.value += step,
		PressAction::Reset => next.value = next.start,
		PressAction::Nothing => {}
	}
	next
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

async fn press(
	instance: &Instance,
	settings: &CounterSettings,
	action: PressAction,
	step: isize,
) -> OpenActionResult<()> {
	if action == PressAction::Nothing {
		return Ok(());
	}
	let next = apply(settings, action, step);
	instance.set_settings(&next).await?;
	refresh_title(instance, &next).await
}

struct Hold {
	claimed: Arc<AtomicBool>,
	task: JoinHandle<()>,
}

pub struct Counter {
	holds: Mutex<HashMap<InstanceId, Hold>>,
}

impl Counter {
	pub fn new() -> Self {
		Self {
			holds: Mutex::new(HashMap::new()),
		}
	}

	fn take(&self, instance_id: &str) -> Option<Hold> {
		self.holds.lock().unwrap().remove(instance_id)
	}
}

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

	async fn will_disappear(
		&self,
		instance: &Instance,
		_settings: &Self::Settings,
	) -> OpenActionResult<()> {
		if let Some(hold) = self.take(&instance.instance_id) {
			hold.task.abort();
		}
		Ok(())
	}

	async fn did_receive_settings(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		refresh_title(instance, settings).await
	}

	async fn key_down(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		if let Some(hold) = self.take(&instance.instance_id) {
			hold.task.abort();
		}
		let claimed = Arc::new(AtomicBool::new(false));
		let task = tokio::spawn({
			let claimed = claimed.clone();
			let instance_id = instance.instance_id.clone();
			let settings = settings.clone();
			async move {
				tokio::time::sleep(LONG_PRESS).await;
				if claimed.swap(true, Ordering::SeqCst) {
					return;
				}
				let Some(instance) = get_instance(instance_id).await else {
					return;
				};
				let result = async {
					press(&instance, &settings, settings.long_action, settings.long_step).await?;
					instance.show_ok().await
				}
				.await;
				if let Err(error) = result {
					log::error!("Counter long press failed: {error}");
				}
			}
		});
		self.holds
			.lock()
			.unwrap()
			.insert(instance.instance_id.clone(), Hold { claimed, task });
		Ok(())
	}

	async fn key_up(&self, instance: &Instance, settings: &Self::Settings) -> OpenActionResult<()> {
		if let Some(hold) = self.take(&instance.instance_id) {
			if hold.claimed.swap(true, Ordering::SeqCst) {
				return Ok(());
			}
			hold.task.abort();
		}
		press(instance, settings, settings.short_action, settings.step).await
	}
}
