use crate::config::{Config, redact};
use crate::foundry::{EVENT_HOOK, REQUEST_TYPE};

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore, broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

const AUTH_TIMEOUT: Duration = Duration::from_secs(8);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_INFLIGHT: usize = 64;
const BACKOFF: [u64; 6] = [1, 2, 4, 8, 15, 30];
const EVENT_CAPACITY: usize = 64;

#[derive(Debug)]
pub enum RelayError {
	Disconnected,
	Timeout,
	Remote(String),
}

impl std::fmt::Display for RelayError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Disconnected => write!(f, "relay disconnected"),
			Self::Timeout => write!(f, "relay request timed out"),
			Self::Remote(m) => write!(f, "{m}"),
		}
	}
}

#[derive(Clone, Deserialize)]
pub struct ActorRef {
	pub uuid: String,
	pub name: String,
}

pub struct Relay {
	config: Arc<RwLock<Config>>,
	pub config_changed: Notify,
	pub session_started: Notify,
	pub session_ended: Notify,
	pub events: broadcast::Sender<Value>,
	resolved_client_id: RwLock<Option<String>>,
	companion: RwLock<Option<String>>,
	tx: Mutex<Option<mpsc::Sender<String>>>,
	pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
	inflight: Semaphore,
	connected: AtomicBool,
	seq: AtomicU64,
}

impl Relay {
	pub fn new(config: Arc<RwLock<Config>>) -> Self {
		Self {
			config,
			config_changed: Notify::new(),
			session_started: Notify::new(),
			session_ended: Notify::new(),
			events: broadcast::channel(EVENT_CAPACITY).0,
			resolved_client_id: RwLock::new(None),
			companion: RwLock::new(None),
			tx: Mutex::new(None),
			pending: Mutex::new(HashMap::new()),
			inflight: Semaphore::new(MAX_INFLIGHT),
			connected: AtomicBool::new(false),
			seq: AtomicU64::new(0),
		}
	}

	pub fn is_connected(&self) -> bool {
		self.connected.load(Ordering::Relaxed)
	}

	pub async fn resolved_client_id(&self) -> Option<String> {
		self.resolved_client_id.read().await.clone()
	}

	pub async fn companion(&self) -> Option<String> {
		self.companion.read().await.clone()
	}

	pub async fn set_companion(&self, version: Option<String>) {
		*self.companion.write().await = version;
	}

	fn request_id(&self) -> String {
		format!(
			"sd-{:x}-{}",
			std::process::id(),
			self.seq.fetch_add(1, Ordering::Relaxed)
		)
	}

	pub async fn run_forever(self: Arc<Self>) {
		let mut attempt = 0usize;
		loop {
			let config = self.config.read().await.clone();
			if !config.is_complete() {
				log::info!("relay config incomplete; waiting for settings");
				self.config_changed.notified().await;
				attempt = 0;
				continue;
			}

			match self.session(&config).await {
				Ok(()) => {
					log::info!("relay session ended");
					attempt = 0;
				}
				Err(error) => {
					log::warn!("relay session failed: {error}");
					attempt = (attempt + 1).min(BACKOFF.len() - 1);
				}
			}

			self.teardown().await;

			let delay = Duration::from_secs(BACKOFF[attempt]);
			tokio::select! {
				_ = tokio::time::sleep(delay) => {}
				_ = self.config_changed.notified() => { attempt = 0; }
			}
		}
	}

	async fn session(&self, config: &Config) -> Result<(), String> {
		let url = config.socket_url();
		log::info!("relay connecting: {url}");

		let (mut stream, _) = tokio_tungstenite::connect_async(&url)
			.await
			.map_err(|e| format!("connect: {e}"))?;

		let mut auth = json!({ "type": "auth", "token": config.api_key.trim() });
		if !config.client_id.trim().is_empty() {
			auth["clientId"] = json!(config.client_id.trim());
		}
		log::debug!(
			"relay auth as clientId={} token={}",
			if config.client_id.trim().is_empty() {
				"<from key>"
			} else {
				config.client_id.trim()
			},
			redact(config.api_key.trim())
		);
		stream
			.send(Message::Text(auth.to_string().into()))
			.await
			.map_err(|e| format!("send auth: {e}"))?;

		loop {
			let frame = tokio::time::timeout(AUTH_TIMEOUT, stream.next())
				.await
				.map_err(|_| "auth timed out".to_string())?;
			match frame {
				Some(Ok(Message::Text(text))) => {
					let value: Value = serde_json::from_str(&text)
						.map_err(|e| format!("auth reply not json: {e}"))?;
					match value.get("type").and_then(Value::as_str) {
						Some("connected") => {
							let resolved = value
								.get("clientId")
								.and_then(Value::as_str)
								.map(str::to_string);
							log::info!(
								"relay connected (clientId={})",
								resolved.as_deref().unwrap_or("?")
							);
							*self.resolved_client_id.write().await = resolved;
							if let Some(types) =
								value.get("supportedTypes").and_then(Value::as_array)
							{
								if !types.iter().any(|t| t.as_str() == Some("search")) {
									log::warn!("relay does not advertise 'search'");
								}
								if !types.iter().any(|t| t.as_str() == Some(REQUEST_TYPE)) {
									log::error!(
										"relay does not support the '{REQUEST_TYPE}' request type; it needs \
										 the foundryvtt-rest-api-relay fork patch"
									);
								}
							}
							if !value
								.get("eventChannels")
								.and_then(Value::as_array)
								.is_some_and(|c| c.iter().any(|t| t.as_str() == Some("hooks")))
							{
								log::warn!("relay does not advertise the 'hooks' event channel");
							}
							break;
						}
						Some("error") => {
							return Err(format!(
								"auth rejected: {}",
								value
									.get("error")
									.and_then(Value::as_str)
									.unwrap_or("unknown")
							));
						}
						_ => continue,
					}
				}
				Some(Ok(Message::Close(frame))) => {
					return Err(match frame {
						Some(f) => format!("closed during auth: {} {}", f.code, f.reason),
						None => "closed during auth".to_string(),
					});
				}
				Some(Ok(_)) => continue,
				Some(Err(e)) => return Err(format!("auth read: {e}")),
				None => return Err("stream ended during auth".to_string()),
			}
		}

		let subscribe = json!({
			"type": "subscribe",
			"channel": "hooks",
			"requestId": self.request_id(),
		});
		stream
			.send(Message::Text(subscribe.to_string().into()))
			.await
			.map_err(|e| format!("send subscribe: {e}"))?;

		let (tx, mut rx) = mpsc::channel::<String>(64);
		*self.tx.lock().await = Some(tx);
		self.connected.store(true, Ordering::Relaxed);
		self.session_started.notify_waiters();

		loop {
			tokio::select! {
				frame = stream.next() => match frame {
					Some(Ok(Message::Text(text))) => self.dispatch(&text).await,
					Some(Ok(Message::Close(f))) => {
						log::info!("relay closed: {f:?}");
						return Ok(());
					}
					Some(Ok(_)) => {}
					Some(Err(e)) => return Err(format!("read: {e}")),
					None => return Ok(()),
				},
				outgoing = rx.recv() => match outgoing {
					Some(text) => {
						if let Err(e) = stream.send(Message::Text(text.into())).await {
							return Err(format!("write: {e}"));
						}
					}
					None => return Ok(()),
				},
				_ = self.config_changed.notified() => {
					log::info!("relay config changed; reconnecting");
					return Ok(());
				}
			}
		}
	}

	async fn dispatch(&self, text: &str) {
		let Ok(value) = serde_json::from_str::<Value>(text) else {
			log::warn!("relay sent non-json frame");
			return;
		};
		match value.get("type").and_then(Value::as_str) {
			Some("hook-event") => {
				if value.get("hook").and_then(Value::as_str) == Some(EVENT_HOOK) {
					match value.pointer("/data/data/args/0") {
						Some(payload) => {
							let _ = self.events.send(payload.clone());
						}
						None => log::warn!("{EVENT_HOOK} event without a payload"),
					}
				}
				return;
			}
			Some("subscribed") => {
				log::debug!(
					"relay subscribed to '{}'",
					value.get("channel").and_then(Value::as_str).unwrap_or("?")
				);
				return;
			}
			_ => {}
		}
		let Some(id) = value.get("requestId").and_then(Value::as_str) else {
			if value.get("type").and_then(Value::as_str) == Some("error") {
				log::warn!(
					"relay error: {}",
					value
						.get("error")
						.and_then(Value::as_str)
						.unwrap_or("unknown")
				);
			}
			return;
		};
		match self.pending.lock().await.remove(id) {
			Some(sender) => {
				let _ = sender.send(value);
			}
			None => log::debug!("relay reply for unknown requestId {id}"),
		}
	}

	async fn teardown(&self) {
		let was_connected = self.connected.swap(false, Ordering::Relaxed);
		*self.tx.lock().await = None;
		*self.companion.write().await = None;
		let drained: Vec<_> = self.pending.lock().await.drain().collect();
		if !drained.is_empty() {
			log::debug!("relay dropped {} in-flight request(s)", drained.len());
		}
		if was_connected {
			self.session_ended.notify_waiters();
		}
	}

	pub async fn request(
		&self,
		kind: &str,
		mut params: Value,
		timeout: Duration,
	) -> Result<Value, RelayError> {
		let _permit = self
			.inflight
			.acquire()
			.await
			.map_err(|_| RelayError::Disconnected)?;

		let sender = {
			let guard = self.tx.lock().await;
			guard.clone().ok_or(RelayError::Disconnected)?
		};

		let id = self.request_id();
		params["type"] = json!(kind);
		params["requestId"] = json!(id);

		let (otx, orx) = oneshot::channel();
		self.pending.lock().await.insert(id.clone(), otx);

		if sender.send(params.to_string()).await.is_err() {
			self.pending.lock().await.remove(&id);
			return Err(RelayError::Disconnected);
		}

		match tokio::time::timeout(timeout, orx).await {
			Ok(Ok(value)) => {
				if let Some(error) = value.get("error").and_then(Value::as_str) {
					return Err(RelayError::Remote(error.to_string()));
				}
				Ok(value)
			}
			Ok(Err(_)) => Err(RelayError::Disconnected),
			Err(_) => {
				self.pending.lock().await.remove(&id);
				Err(RelayError::Timeout)
			}
		}
	}

	pub async fn call(
		&self,
		action: &str,
		args: Value,
		timeout: Duration,
	) -> Result<Value, RelayError> {
		let reply = self
			.request(
				REQUEST_TYPE,
				json!({ "action": action, "args": args }),
				timeout,
			)
			.await?;
		Ok(reply.get("result").cloned().unwrap_or(Value::Null))
	}

	pub async fn search_actors(&self) -> Result<Vec<ActorRef>, RelayError> {
		let reply = self
			.request(
				"search",
				json!({
					"query": "",
					"filter": "documentType:Actor",
					"excludeCompendiums": true,
					"minified": true,
					"limit": 500,
				}),
				REQUEST_TIMEOUT,
			)
			.await?;

		let mut actors: Vec<ActorRef> = reply
			.get("results")
			.and_then(Value::as_array)
			.map(|rows| {
				rows.iter()
					.filter_map(|row| serde_json::from_value(row.clone()).ok())
					.collect()
			})
			.unwrap_or_default();
		actors.sort_by_key(|a| a.name.to_lowercase());
		Ok(actors)
	}
}
