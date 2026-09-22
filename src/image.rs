use crate::foundry::ArtSource;

use base64::Engine;
use std::collections::HashMap;
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
	fetched: Instant,
}

impl ArtEntry {
	pub fn new(closed: String, open: String, offline: String, name: String) -> Self {
		Self {
			closed,
			open,
			offline,
			name,
			fetched: Instant::now(),
		}
	}

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

#[derive(Default)]
pub struct ArtCache {
	entries: RwLock<HashMap<ArtKey, ArtEntry>>,
	order: RwLock<Vec<ArtKey>>,
}

impl ArtCache {
	pub async fn get(&self, key: &ArtKey) -> Option<ArtEntry> {
		let entry = self.entries.read().await.get(key).cloned()?;
		if entry.fetched.elapsed() > TTL {
			self.remove(key).await;
			return None;
		}
		Some(entry)
	}

	pub async fn insert(&self, key: ArtKey, entry: ArtEntry) {
		self.entries.write().await.insert(key.clone(), entry);

		let mut order = self.order.write().await;
		order.retain(|k| k != &key);
		order.push(key);

		while order.len() > CAPACITY {
			let evicted = order.remove(0);
			self.entries.write().await.remove(&evicted);
		}
	}

	pub async fn remove(&self, key: &ArtKey) {
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
