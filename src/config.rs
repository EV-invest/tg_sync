use std::collections::HashMap;

use color_eyre::eyre::{Result, bail, eyre};

ev::settings! {
	/// Env-only configuration (no config file, no hot reload) — mirrors concierge.
	///
	/// The topic map and the chat id are authored in gitops' `flake.nix` and ride
	/// as plain env vars; only the bot token is a secret, and it arrives through
	/// the `kubernetes-tg-sync` Secret the rpi5 host renders from sops.
	pub struct Config {
		#[secret]
		telegram_bot_token: String,
		/// Bot API chat id of the supergroup, e.g. `-1001234567890`.
		tg_chat_id: i64,
		/// `<thread_id>:<slug>` pairs. The General topic is thread id 1.
		tg_topic_map: Vec<String> = "",
		/// Inline-relay ceiling. Telegram's `getFile` refuses over 20 MB and Discord's
		/// free tier refuses over 10 MB, so the real ceiling is below both; anything
		/// larger is mirrored as a stub + source link, never dropped.
		tg_media_max_bytes: u64 = "8388608",
		/// Long-poll window handed to `getUpdates`. The HTTP timeout is derived from it.
		tg_poll_timeout_secs: u64 = "25",
		bind: std::net::SocketAddr = "0.0.0.0:55680",
		app_env: String = "development",
	}
}

/// Telegram omits `message_thread_id` on the General topic (and on every message
/// of a non-forum group); id 1 is what the client's own deep links use for it, so
/// the map can name it like any other topic.
pub const GENERAL_THREAD_ID: i64 = 1;
const WEBHOOK_PREFIX: &str = "DISCORD_WEBHOOK_TG_";
const UNMAPPED_VAR: &str = "DISCORD_WEBHOOK_TG_UNMAPPED";
/// Thread id → webhook URL, with the catch-all every unmapped topic lands in.
///
/// Built once at boot so a missing webhook is a boot failure rather than a message
/// discovered undeliverable at 3am.
pub struct Router {
	by_thread: HashMap<i64, String>,
	unmapped: String,
}
impl Router {
	pub fn from_env(topic_map: &[String]) -> Result<Self> {
		let unmapped = webhook(UNMAPPED_VAR)?;
		let mut by_thread = HashMap::new();
		for entry in topic_map {
			let (id, slug) = entry.split_once(':').ok_or_else(|| eyre!("TG_TOPIC_MAP entry {entry:?} is not `<thread_id>:<slug>`"))?;
			let id: i64 = id.trim().parse().map_err(|e| eyre!("TG_TOPIC_MAP entry {entry:?} has a non-numeric thread id: {e}"))?;
			let var = format!("{WEBHOOK_PREFIX}{}", slug.trim().to_uppercase().replace('-', "_"));
			if by_thread.insert(id, webhook(&var)?).is_some() {
				bail!("TG_TOPIC_MAP maps thread id {id} twice");
			}
		}
		Ok(Self { by_thread, unmapped })
	}

	/// `None` (General topic / non-forum group) resolves through [`GENERAL_THREAD_ID`].
	pub fn route(&self, thread_id: Option<i64>) -> &str {
		self.by_thread.get(&thread_id.unwrap_or(GENERAL_THREAD_ID)).unwrap_or(&self.unmapped)
	}
}

fn webhook(var: &str) -> Result<String> {
	match std::env::var(var) {
		Ok(url) if url.starts_with("https://") => Ok(url),
		Ok(url) => bail!(
			"{var} is not an https webhook URL (got {} chars starting {:?})",
			url.len(),
			url.chars().take(8).collect::<String>()
		),
		Err(e) => bail!("{var} is unset — every mapped topic needs a Discord webhook: {e}"),
	}
}
