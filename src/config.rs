use serde::{Deserialize, Serialize};

pub const OFFLINE_COLOR: &str = "#6b6b6b";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Indicator {
	Border,
	Svg,
	Title,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
	pub relay_url: String,
	pub api_key: String,
	pub client_id: String,
	pub poll_ms: u64,
	pub indicator: Indicator,
	pub border_color: String,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			relay_url: String::new(),
			api_key: String::new(),
			client_id: String::new(),
			poll_ms: 5000,
			indicator: Indicator::Border,
			border_color: "#ff6400".to_string(),
		}
	}
}

impl Config {
	/// A scoped relay key can carry its own clientId, in which case the relay resolves it
	/// for us and the setting may be left blank.
	pub fn is_complete(&self) -> bool {
		!self.relay_url.trim().is_empty() && !self.api_key.trim().is_empty()
	}

	pub fn connection_differs(&self, other: &Self) -> bool {
		self.relay_url != other.relay_url
			|| self.api_key != other.api_key
			|| self.client_id != other.client_id
	}

	pub fn appearance_differs(&self, other: &Self) -> bool {
		self.indicator != other.indicator || self.border_color != other.border_color
	}

	pub fn socket_url(&self) -> String {
		let base = self.relay_url.trim().trim_end_matches('/');
		let base = match base.strip_prefix("https://") {
			Some(rest) => format!("wss://{rest}"),
			None => match base.strip_prefix("http://") {
				Some(rest) => format!("ws://{rest}"),
				None => base.to_string(),
			},
		};
		let client_id = self.client_id.trim();
		if client_id.is_empty() {
			format!("{base}/ws/api")
		} else {
			format!("{base}/ws/api?clientId={}", urlencode(client_id))
		}
	}
}

impl std::fmt::Debug for Config {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Config")
			.field("relay_url", &self.relay_url)
			.field("api_key", &redact(&self.api_key))
			.field("client_id", &self.client_id)
			.field("poll_ms", &self.poll_ms)
			.field("border_color", &self.border_color)
			.finish()
	}
}

pub fn redact(secret: &str) -> String {
	let n = secret.chars().count();
	if n == 0 {
		"<unset>".to_string()
	} else if n <= 4 {
		"<redacted>".to_string()
	} else {
		format!("<redacted:{}>", &secret[secret.len() - 4..])
	}
}

fn urlencode(raw: &str) -> String {
	let mut out = String::with_capacity(raw.len());
	for byte in raw.bytes() {
		match byte {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
				out.push(byte as char)
			}
			_ => out.push_str(&format!("%{byte:02X}")),
		}
	}
	out
}

pub fn apply_env_fallback(mut config: Config) -> Config {
	fn env(key: &str) -> Option<String> {
		std::env::var(key).ok().filter(|v| !v.trim().is_empty())
	}

	if config.relay_url.trim().is_empty()
		&& let Some(v) = env("FOUNDRY_RELAY_URL")
	{
		config.relay_url = v;
	}
	if config.api_key.trim().is_empty()
		&& let Some(v) = env("FOUNDRY_RELAY_KEY")
	{
		config.api_key = v;
	}
	if config.client_id.trim().is_empty()
		&& let Some(v) = env("FOUNDRY_RELAY_CLIENT_ID")
	{
		config.client_id = v;
	}
	if let Some(v) = env("FOUNDRY_RELAY_POLL_MS").and_then(|v| v.trim().parse::<u64>().ok()) {
		config.poll_ms = v;
	}
	config
}
