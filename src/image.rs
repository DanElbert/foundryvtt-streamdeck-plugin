use crate::foundry::ArtSource;

use base64::Engine;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const TTL: Duration = Duration::from_secs(600);
const CAPACITY: usize = 32;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ArtKey {
	pub uuid: String,
	pub source: ArtSource,
	pub open_color: String,
}

#[derive(Clone)]
pub struct ArtEntry {
	pub closed: String,
	pub open: String,
	pub offline: String,
	pub name: String,
}

impl ArtEntry {
	pub fn variant(&self, open: bool, online: bool) -> &str {
		if !online {
			&self.offline
		} else if open {
			&self.open
		} else {
			&self.closed
		}
	}
}

pub type ArtCache = TtlCache<ArtKey, ArtEntry>;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ConditionKey {
	pub id: String,
	pub accent: String,
}

#[derive(Clone)]
pub struct ConditionArt {
	pub name: String,
	pub off: String,
	pub on: String,
	pub mixed: String,
	pub none: String,
	pub offline: String,
}

pub struct TtlCache<K, V> {
	entries: RwLock<HashMap<K, (V, Instant)>>,
	order: RwLock<Vec<K>>,
}

impl<K, V> Default for TtlCache<K, V> {
	fn default() -> Self {
		Self {
			entries: RwLock::new(HashMap::new()),
			order: RwLock::new(Vec::new()),
		}
	}
}

impl<K: Clone + Eq + Hash, V: Clone> TtlCache<K, V> {
	pub async fn get(&self, key: &K) -> Option<V> {
		let (value, fetched) = self.entries.read().await.get(key).cloned()?;
		if fetched.elapsed() > TTL {
			self.remove(key).await;
			return None;
		}
		Some(value)
	}

	pub async fn insert(&self, key: K, value: V) {
		self.entries
			.write()
			.await
			.insert(key.clone(), (value, Instant::now()));

		let mut order = self.order.write().await;
		order.retain(|k| k != &key);
		order.push(key);

		while order.len() > CAPACITY {
			let evicted = order.remove(0);
			self.entries.write().await.remove(&evicted);
		}
	}

	pub async fn remove(&self, key: &K) {
		self.entries.write().await.remove(key);
		self.order.write().await.retain(|k| k != key);
	}

	pub async fn clear(&self) {
		self.entries.write().await.clear();
		self.order.write().await.clear();
	}
}

pub fn wrap_svg(art: &str, color: Option<&str>) -> String {
	let border = match color {
		Some(c) => format!(
			r#"<rect x="5" y="5" width="134" height="134" rx="12" fill="none" stroke="{c}" stroke-width="10"/>"#
		),
		None => String::new(),
	};
	let svg = format!(
		r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 144 144" width="144" height="144"><image href="{art}" x="0" y="0" width="144" height="144" preserveAspectRatio="xMidYMid meet"/>{border}</svg>"#
	);
	format!(
		"data:image/svg+xml;base64,{}",
		base64::engine::general_purpose::STANDARD.encode(svg)
	)
}
