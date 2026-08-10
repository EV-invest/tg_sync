//! Telegram forum Topic → the Discord channel that mirrors it.
//!
//! There is no declarative map: a Topic gets its channel the first time it says
//! anything, created under the configured category and named after the Topic.
//!
//! The binding lives in Discord, not here — each mirror channel carries a
//! `tg:<thread_id>` marker in its description, so lookup survives a rename on
//! either side and the bridge stays stateless across restarts.

use std::collections::HashMap;

use color_eyre::eyre::{Context, Result};

use crate::discord::Discord;

/// Telegram omits `message_thread_id` on the General topic (and on every message
/// of a non-forum group); id 1 is what the client's own deep links use for it.
pub const GENERAL_THREAD_ID: i64 = 1;
/// Discord's channel-name ceiling is 100; leave room for a disambiguating suffix.
const NAME_LIMIT: usize = 90;

pub struct Channels {
	discord: Discord,
	guild_id: String,
	category_id: String,
	by_thread: HashMap<i64, Mirror>,
}
impl Channels {
	/// The guild is read off the category rather than configured: one id to author
	/// in gitops, and it cannot disagree with itself.
	pub async fn try_new(discord: Discord, category_id: String) -> Result<Self> {
		let category = discord.channel(&category_id).await.with_context(|| format!("failed to read the mirror category {category_id}"))?;
		let guild_id = category.guild_id.clone().ok_or_else(|| color_eyre::eyre::eyre!("channel {category_id} is not inside a guild"))?;
		tracing::info!(guild = %guild_id, category = %category.name, "mirroring into");
		Ok(Self {
			discord,
			guild_id,
			category_id,
			by_thread: HashMap::new(),
		})
	}

	/// The webhook for `thread`, creating the channel if this Topic is new and
	/// renaming it if Telegram's name has moved on. `tg_name` is whatever the
	/// update revealed — the Bot API has no way to list Topics, so a Topic that has
	/// not yet shown its name is mirrored as `topic-<id>` and renamed later.
	pub async fn webhook(&mut self, thread: i64, tg_name: Option<&str>) -> Result<&str> {
		let desired = slug(tg_name, thread);
		if !self.by_thread.contains_key(&thread) {
			let mirror = self.open(thread, &desired).await?;
			self.by_thread.insert(thread, mirror);
		}
		// A Topic renamed in Telegram renames its mirror, so the two never drift.
		// Only act on a name we actually learned: `topic-<id>` is a placeholder, not
		// a rename, and must never overwrite a real name.
		let mirror = self.by_thread.get_mut(&thread).expect("inserted above");
		if tg_name.is_some() && mirror.name != desired {
			self.discord
				.rename_channel(&mirror.channel_id, &desired, &marker(thread, tg_name))
				.await
				.with_context(|| format!("failed to rename the mirror of topic {thread}"))?;
			tracing::info!(thread, from = %mirror.name, to = %desired, "renamed the mirror channel");
			mirror.name = desired;
		}
		Ok(&mirror.webhook)
	}

	async fn open(&self, thread: i64, desired: &str) -> Result<Mirror> {
		let marker = format!("tg:{thread}");
		let existing = self
			.discord
			.guild_channels(&self.guild_id)
			.await
			.context("failed to list the guild's channels")?
			.into_iter()
			.find(|c| c.parent_id.as_deref() == Some(self.category_id.as_str()) && c.topic.as_deref().is_some_and(|t| t.starts_with(&marker)));

		let channel = match existing {
			Some(channel) => {
				tracing::info!(thread, channel = %channel.name, "adopted an existing mirror channel");
				channel
			}
			None => {
				let channel = self
					.discord
					.create_channel(&self.guild_id, desired, &self.category_id, &marker)
					.await
					.with_context(|| format!("failed to create a mirror channel for topic {thread}"))?;
				tracing::info!(thread, channel = %channel.name, "created a mirror channel");
				channel
			}
		};
		let webhook = self
			.discord
			.webhook_for(&channel.id)
			.await
			.with_context(|| format!("failed to mint the webhook of #{}", channel.name))?;
		Ok(Mirror {
			channel_id: channel.id,
			webhook,
			name: channel.name,
		})
	}

	pub fn discord(&self) -> &Discord {
		&self.discord
	}
}

struct Mirror {
	channel_id: String,
	webhook: String,
	name: String,
}

fn marker(thread: i64, tg_name: Option<&str>) -> String {
	match tg_name {
		Some(name) => format!("tg:{thread} — mirrored from the Telegram topic \"{name}\""),
		None => format!("tg:{thread}"),
	}
}

/// Discord channel names are lowercase and dash-separated. An unnamed Topic keeps
/// a stable `topic-<id>` placeholder so the channel is findable before its name
/// is known, and readable enough to recognise if it never is.
fn slug(tg_name: Option<&str>, thread: i64) -> String {
	let fallback = || {
		if thread == GENERAL_THREAD_ID { "general".to_string() } else { format!("topic-{thread}") }
	};
	let Some(name) = tg_name else { return fallback() };

	let mut out = String::new();
	for c in name.chars().flat_map(char::to_lowercase) {
		if c.is_alphanumeric() {
			out.push(c);
		} else if !out.ends_with('-') {
			out.push('-');
		}
	}
	let out: String = out.trim_matches('-').chars().take(NAME_LIMIT).collect();
	// Emoji-only or punctuation-only Topic names slug to nothing.
	if out.is_empty() { fallback() } else { out }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn slugs_are_discord_safe_and_never_empty() {
		assert_eq!(slug(Some("memes(trading)"), 7), "memes-trading");
		assert_eq!(slug(Some("  Fund Structure  "), 7), "fund-structure");
		// Non-latin names survive: Discord accepts unicode channel names.
		assert_eq!(slug(Some("Отчёты"), 7), "отчёты");
		assert_eq!(slug(Some("🔥🔥"), 7), "topic-7");
		assert_eq!(slug(None, 7), "topic-7");
		assert_eq!(slug(None, GENERAL_THREAD_ID), "general");
		assert!(slug(Some(&"x".repeat(300)), 7).chars().count() <= NAME_LIMIT);
	}
}
