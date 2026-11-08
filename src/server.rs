use crate::config::Config;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::futures::Notified;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore, broadcast, mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior, interval, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_INFLIGHT: usize = 64;
const BIND_BACKOFF: [u64; 6] = [1, 2, 4, 8, 15, 30];
const EVENT_CAPACITY: usize = 64;

pub const CLOSE_BAD_HELLO: u16 = 4001;
pub const CLOSE_BAD_TOKEN: u16 = 4002;
pub const CLOSE_REPLACED: u16 = 4003;
const CLOSE_GOING_AWAY: u16 = 1001;

fn ping_interval() -> Duration {
	std::env::var("FOUNDRY_SD_PING_SECS")
		.ok()
		.and_then(|v| v.parse::<u64>().ok())
		.filter(|v| *v > 0)
		.map(Duration::from_secs)
		.unwrap_or(Duration::from_secs(20))
}

#[derive(Debug)]
pub enum ServerError {
	Disconnected,
	Timeout,
	Remote(String),
}

impl std::fmt::Display for ServerError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Disconnected => write!(f, "companion disconnected"),
			Self::Timeout => write!(f, "companion request timed out"),
			Self::Remote(m) => write!(f, "{m}"),
		}
	}
}

struct Authed {
	stream: WebSocketStream<TcpStream>,
	version: String,
	world: String,
	peer: SocketAddr,
}

type SessionFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub struct Server {
	config: Arc<RwLock<Config>>,
	pub config_changed: Notify,
	pub session_started: Notify,
	pub session_ended: Notify,
	pub listen_changed: Notify,
	pub events: broadcast::Sender<Value>,
	companion: RwLock<Option<String>>,
	world: RwLock<Option<String>>,
	listen_error: RwLock<Option<String>>,
	tx: Mutex<Option<mpsc::Sender<String>>>,
	pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
	inflight: Semaphore,
	connected: AtomicBool,
	listening: AtomicBool,
	seq: AtomicU64,
}

impl Server {
	pub fn new(config: Arc<RwLock<Config>>) -> Self {
		Self {
			config,
			config_changed: Notify::new(),
			session_started: Notify::new(),
			session_ended: Notify::new(),
			listen_changed: Notify::new(),
			events: broadcast::channel(EVENT_CAPACITY).0,
			companion: RwLock::new(None),
			world: RwLock::new(None),
			listen_error: RwLock::new(None),
			tx: Mutex::new(None),
			pending: Mutex::new(HashMap::new()),
			inflight: Semaphore::new(MAX_INFLIGHT),
			connected: AtomicBool::new(false),
			listening: AtomicBool::new(false),
			seq: AtomicU64::new(0),
		}
	}

	pub fn is_connected(&self) -> bool {
		self.connected.load(Ordering::Relaxed)
	}

	pub fn is_listening(&self) -> bool {
		self.listening.load(Ordering::Relaxed)
	}

	pub async fn listen_error(&self) -> Option<String> {
		self.listen_error.read().await.clone()
	}

	pub async fn companion(&self) -> Option<String> {
		self.companion.read().await.clone()
	}

	pub async fn world(&self) -> Option<String> {
		self.world.read().await.clone()
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
				log::info!("no token configured; not listening");
				self.config_changed.notified().await;
				attempt = 0;
				continue;
			}

			let changed = self.config_changed.notified();
			tokio::pin!(changed);
			changed.as_mut().enable();

			let addr = format!("{}:{}", config.bind.trim(), config.port);
			let listener = match TcpListener::bind(&addr).await {
				Ok(listener) => listener,
				Err(error) => {
					let message = format!("cannot listen on {addr}: {error}");
					log::error!("{message}");
					*self.listen_error.write().await = Some(message);
					self.listening.store(false, Ordering::Relaxed);
					self.listen_changed.notify_waiters();
					attempt = (attempt + 1).min(BIND_BACKOFF.len() - 1);
					tokio::select! {
						_ = tokio::time::sleep(Duration::from_secs(BIND_BACKOFF[attempt])) => {}
						_ = &mut changed => { attempt = 0; }
					}
					continue;
				}
			};
			attempt = 0;
			*self.listen_error.write().await = None;
			self.listening.store(true, Ordering::Relaxed);
			self.listen_changed.notify_waiters();
			log::info!("listening on ws://{addr}");

			self.serve(listener, &config, changed).await;
			self.teardown().await;
			self.listening.store(false, Ordering::Relaxed);
			self.listen_changed.notify_waiters();
		}
	}

	async fn serve(
		&self,
		listener: TcpListener,
		config: &Config,
		mut changed: Pin<&mut Notified<'_>>,
	) {
		let token: Arc<str> = config.token.trim().into();
		let (ready_tx, mut ready_rx) = mpsc::channel::<Authed>(4);
		let end = Notify::new();
		let end_code = AtomicU16::new(CLOSE_GOING_AWAY);
		let mut session: Option<SessionFuture<'_>> = None;

		loop {
			tokio::select! {
				accepted = listener.accept() => match accepted {
					Ok((tcp, peer)) => {
						log::debug!("connection from {peer}");
						tokio::spawn(handshake(tcp, peer, token.clone(), ready_tx.clone()));
					}
					Err(error) => {
						log::warn!("accept failed: {error}");
						tokio::time::sleep(Duration::from_millis(100)).await;
					}
				},
				Some(authed) = ready_rx.recv() => {
					if let Some(old) = session.take() {
						log::info!("companion replaced by a new connection from {}", authed.peer);
						end_code.store(CLOSE_REPLACED, Ordering::Relaxed);
						end.notify_one();
						old.await;
						self.teardown().await;
					}
					let (tx, rx) = mpsc::channel::<String>(64);
					*self.tx.lock().await = Some(tx);
					*self.companion.write().await = Some(authed.version.clone());
					*self.world.write().await = Some(authed.world.clone());
					self.connected.store(true, Ordering::Relaxed);
					log::info!(
						"companion v{} connected from {} (world {})",
						authed.version,
						authed.peer,
						if authed.world.is_empty() { "?" } else { &authed.world }
					);
					session = Some(Box::pin(self.session(authed.stream, rx, &end, &end_code)));
					self.session_started.notify_waiters();
				},
				_ = async { session.as_mut().unwrap().await }, if session.is_some() => {
					session = None;
					self.teardown().await;
				},
				_ = &mut changed => {
					log::info!("config changed; closing listener");
					if let Some(current) = session.take() {
						end_code.store(CLOSE_GOING_AWAY, Ordering::Relaxed);
						end.notify_one();
						current.await;
					}
					return;
				}
			}
		}
	}

	async fn session(
		&self,
		mut stream: WebSocketStream<TcpStream>,
		mut rx: mpsc::Receiver<String>,
		end: &Notify,
		end_code: &AtomicU16,
	) {
		let every = ping_interval();
		let idle_limit = every * 2 + Duration::from_secs(5);
		let mut ping = interval(every);
		ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
		ping.tick().await;
		let mut last_rx = Instant::now();

		loop {
			tokio::select! {
				frame = stream.next() => {
					last_rx = Instant::now();
					match frame {
						Some(Ok(Message::Text(text))) => self.dispatch(&text).await,
						Some(Ok(Message::Close(frame))) => {
							log::info!("companion closed: {frame:?}");
							return;
						}
						Some(Ok(_)) => {}
						Some(Err(error)) => {
							log::warn!("companion read failed: {error}");
							return;
						}
						None => return,
					}
				},
				outgoing = rx.recv() => match outgoing {
					Some(text) => {
						if let Err(error) = stream.send(Message::Text(text.into())).await {
							log::warn!("companion write failed: {error}");
							return;
						}
					}
					None => return,
				},
				_ = ping.tick() => {
					if last_rx.elapsed() > idle_limit {
						log::warn!("companion unresponsive for {:?}; dropping", last_rx.elapsed());
						return;
					}
					if let Err(error) = stream.send(Message::Ping(Vec::new().into())).await {
						log::warn!("companion ping failed: {error}");
						return;
					}
				},
				_ = end.notified() => {
					let code = end_code.load(Ordering::Relaxed);
					let reason = if code == CLOSE_REPLACED { "replaced" } else { "reconfigured" };
					let _ = timeout(CLOSE_TIMEOUT, stream.send(close_frame(code, reason))).await;
					return;
				}
			}
		}
	}

	async fn dispatch(&self, text: &str) {
		let Ok(value) = serde_json::from_str::<Value>(text) else {
			log::warn!("companion sent non-json frame");
			return;
		};
		match value.get("type").and_then(Value::as_str) {
			Some("event") => {
				let _ = self.events.send(value);
			}
			Some("result") => {
				let Some(id) = value.get("requestId").and_then(Value::as_str) else {
					log::warn!("result without requestId");
					return;
				};
				match self.pending.lock().await.remove(id) {
					Some(sender) => {
						let _ = sender.send(value);
					}
					None => log::debug!("reply for unknown requestId {id}"),
				}
			}
			other => log::debug!("ignoring companion frame of type {other:?}"),
		}
	}

	async fn teardown(&self) {
		let was_connected = self.connected.swap(false, Ordering::Relaxed);
		*self.tx.lock().await = None;
		*self.companion.write().await = None;
		let drained: Vec<_> = self.pending.lock().await.drain().collect();
		if !drained.is_empty() {
			log::debug!("dropped {} in-flight request(s)", drained.len());
		}
		if was_connected {
			self.session_ended.notify_waiters();
		}
	}

	pub async fn call(
		&self,
		action: &str,
		args: Value,
		timeout: Duration,
	) -> Result<Value, ServerError> {
		let _permit = self
			.inflight
			.acquire()
			.await
			.map_err(|_| ServerError::Disconnected)?;

		let sender = {
			let guard = self.tx.lock().await;
			guard.clone().ok_or(ServerError::Disconnected)?
		};

		let id = self.request_id();
		let frame = json!({
			"type": "request",
			"requestId": id,
			"action": action,
			"args": args,
		});

		let (otx, orx) = oneshot::channel();
		self.pending.lock().await.insert(id.clone(), otx);

		if sender.send(frame.to_string()).await.is_err() {
			self.pending.lock().await.remove(&id);
			return Err(ServerError::Disconnected);
		}

		match tokio::time::timeout(timeout, orx).await {
			Ok(Ok(value)) => {
				if let Some(error) = value.get("error").and_then(Value::as_str) {
					return Err(ServerError::Remote(error.to_string()));
				}
				Ok(value.get("result").cloned().unwrap_or(Value::Null))
			}
			Ok(Err(_)) => Err(ServerError::Disconnected),
			Err(_) => {
				self.pending.lock().await.remove(&id);
				Err(ServerError::Timeout)
			}
		}
	}
}

fn close_frame(code: u16, reason: &str) -> Message {
	Message::Close(Some(CloseFrame {
		code: CloseCode::from(code),
		reason: reason.to_string().into(),
	}))
}

fn same_token(offered: &str, expected: &str) -> bool {
	let (a, b) = (offered.as_bytes(), expected.as_bytes());
	if a.len() != b.len() {
		return false;
	}
	a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn handshake(tcp: TcpStream, peer: SocketAddr, token: Arc<str>, ready: mpsc::Sender<Authed>) {
	let mut stream = match timeout(HELLO_TIMEOUT, tokio_tungstenite::accept_async(tcp)).await {
		Ok(Ok(stream)) => stream,
		Ok(Err(error)) => {
			log::debug!("websocket handshake from {peer} failed: {error}");
			return;
		}
		Err(_) => {
			log::debug!("websocket handshake from {peer} timed out");
			return;
		}
	};

	let reject = |mut stream: WebSocketStream<TcpStream>, code: u16, reason: &'static str| async move {
		log::warn!("rejected companion from {peer}: {reason}");
		let _ = timeout(CLOSE_TIMEOUT, stream.send(close_frame(code, reason))).await;
	};

	let text = match timeout(HELLO_TIMEOUT, stream.next()).await {
		Ok(Some(Ok(Message::Text(text)))) => text,
		Ok(_) => return reject(stream, CLOSE_BAD_HELLO, "expected hello").await,
		Err(_) => return reject(stream, CLOSE_BAD_HELLO, "hello timed out").await,
	};
	let hello: Value = match serde_json::from_str(&text) {
		Ok(value) => value,
		Err(_) => return reject(stream, CLOSE_BAD_HELLO, "hello is not json").await,
	};
	if hello.get("type").and_then(Value::as_str) != Some("hello") {
		return reject(stream, CLOSE_BAD_HELLO, "expected hello").await;
	}
	let offered = hello.get("token").and_then(Value::as_str).unwrap_or("");
	if offered.is_empty() || !same_token(offered, &token) {
		return reject(stream, CLOSE_BAD_TOKEN, "bad token").await;
	}

	let version = hello
		.get("version")
		.and_then(Value::as_str)
		.unwrap_or("?")
		.to_string();
	let world = hello
		.get("world")
		.and_then(Value::as_str)
		.unwrap_or("")
		.to_string();

	let welcome = json!({ "type": "connected", "plugin": env!("CARGO_PKG_VERSION") });
	if let Err(error) = stream.send(Message::Text(welcome.to_string().into())).await {
		log::debug!("welcome to {peer} failed: {error}");
		return;
	}

	let _ = ready
		.send(Authed {
			stream,
			version,
			world,
			peer,
		})
		.await;
}
