mod condition;
mod config;
mod connection;
mod counter;
mod events;
mod foundry;
mod image;
mod relay;
mod sheet;

use config::{Config, apply_env_fallback};
use openaction::global_events::*;
use openaction::*;
use std::sync::Arc;
use tokio::sync::RwLock;

struct Handler {
	config: Arc<RwLock<Config>>,
	relay: Arc<relay::Relay>,
	sheet: sheet::Sheet,
	condition: condition::Condition,
}

#[async_trait]
impl GlobalEventHandler for Handler {
	async fn plugin_ready(&self) -> OpenActionResult<()> {
		get_global_settings().await
	}

	async fn did_receive_global_settings(
		&self,
		event: DidReceiveGlobalSettingsEvent,
	) -> OpenActionResult<()> {
		let incoming: Config =
			serde_json::from_value(serde_json::to_value(event.payload.settings)?)
				.unwrap_or_default();
		let merged = apply_env_fallback(incoming);

		let (reconnect, repaint) = {
			let mut guard = self.config.write().await;
			let reconnect = guard.connection_differs(&merged);
			let repaint = guard.appearance_differs(&merged);
			*guard = merged;
			(reconnect, repaint)
		};

		log::debug!("global settings applied: {:?}", self.config.read().await);

		if reconnect {
			self.relay.config_changed.notify_waiters();
		}
		if repaint {
			self.sheet.state.clear_art().await;
			self.condition.state.clear_art().await;
			self.sheet.repaint_visible().await;
			self.condition.repaint_visible().await;
		}
		events::push_connection_state_to_all(&self.sheet, &self.condition).await;
		Ok(())
	}

	async fn system_did_wake_up(&self, _event: SystemDidWakeUpEvent) -> OpenActionResult<()> {
		log::info!("system woke up; forcing relay reconnect");
		self.relay.config_changed.notify_waiters();
		Ok(())
	}
}

#[tokio::main]
async fn main() {
	if let Err(error) = simplelog::TermLogger::init(
		simplelog::LevelFilter::Debug,
		simplelog::Config::default(),
		simplelog::TerminalMode::Stdout,
		simplelog::ColorChoice::Never,
	) {
		eprintln!("failed to initialise logger: {error}");
	}

	let config = Arc::new(RwLock::new(apply_env_fallback(Config::default())));
	let relay = Arc::new(relay::Relay::new(config.clone()));
	let state = Arc::new(sheet::SheetState::default());
	let sheet = sheet::Sheet {
		relay: relay.clone(),
		config: config.clone(),
		state: state.clone(),
	};
	let condition = condition::Condition {
		relay: relay.clone(),
		config: config.clone(),
		state: Arc::new(condition::ConditionState::default()),
	};

	set_global_event_handler(Box::leak(Box::new(Handler {
		config: config.clone(),
		relay: relay.clone(),
		sheet: sheet.clone(),
		condition: condition.clone(),
	})));

	tokio::spawn(relay.clone().run_forever());
	tokio::spawn(events::event_loop(sheet.clone(), condition.clone()));

	{
		let sheet = sheet.clone();
		let condition = condition.clone();
		let relay = relay.clone();
		tokio::spawn(async move {
			loop {
				relay.session_started.notified().await;
				sheet.state.clear_art().await;
				events::resync(sheet.clone(), condition.clone()).await;
				sheet.repaint_visible().await;
				condition.repaint_visible().await;
			}
		});
	}

	{
		let sheet = sheet.clone();
		let condition = condition.clone();
		let relay = relay.clone();
		tokio::spawn(async move {
			loop {
				relay.session_ended.notified().await;
				sheet.repaint_visible().await;
				condition.repaint_visible().await;
				events::push_connection_state_to_all(&sheet, &condition).await;
			}
		});
	}

	register_action(counter::Counter).await;
	register_action(sheet.clone()).await;
	register_action(condition.clone()).await;

	if let Err(error) = run(std::env::args().collect()).await {
		log::error!("plugin exited: {error}");
	}
}
