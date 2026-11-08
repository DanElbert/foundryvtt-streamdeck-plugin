use serde::{Deserialize, Serialize};

pub const OFFLINE_COLOR: &str = "#6b6b6b";
pub const DEFAULT_BIND: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 28415;

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
	pub bind: String,
	pub port: u16,
	pub token: String,
	pub indicator: Indicator,
	pub border_color: String,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			bind: DEFAULT_BIND.to_string(),
			port: DEFAULT_PORT,
			token: String::new(),
			indicator: Indicator::Border,
			border_color: "#ff6400".to_string(),
		}
	}
}

impl Config {
	pub fn is_complete(&self) -> bool {
		!self.token.trim().is_empty() && self.port != 0
	}

	pub fn connection_differs(&self, other: &Self) -> bool {
		self.bind != other.bind || self.port != other.port || self.token != other.token
	}

	pub fn appearance_differs(&self, other: &Self) -> bool {
		self.indicator != other.indicator || self.border_color != other.border_color
	}

	pub fn url(&self) -> String {
		format!("ws://localhost:{}", self.port)
	}
}

impl std::fmt::Debug for Config {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Config")
			.field("bind", &self.bind)
			.field("port", &self.port)
			.field("token", &redact(&self.token))
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

pub fn apply_env_fallback(mut config: Config) -> Config {
	fn env(key: &str) -> Option<String> {
		std::env::var(key).ok().filter(|v| !v.trim().is_empty())
	}

	if config.bind.trim().is_empty() {
		config.bind = env("FOUNDRY_SD_BIND").unwrap_or_else(|| DEFAULT_BIND.to_string());
	}
	if config.port == 0 {
		config.port = env("FOUNDRY_SD_PORT")
			.and_then(|v| v.trim().parse().ok())
			.filter(|p| *p != 0)
			.unwrap_or(DEFAULT_PORT);
	}
	if config.token.trim().is_empty()
		&& let Some(v) = env("FOUNDRY_SD_TOKEN")
	{
		config.token = v;
	}
	config
}
