ev::settings! {
	/// Env-only configuration (no config file, no hot reload) — mirrors concierge.
	///
	/// The chat id and the mirror category are authored in gitops' `flake.nix` and
	/// ride as plain env vars; the two bot tokens arrive through the
	/// `kubernetes-tg-sync` Secret the rpi5 host renders from sops.
	pub struct Config {
		#[secret]
		telegram_bot_token: String,
		/// Needs Manage Channels + Manage Webhooks in the mirror category: tg-sync
		/// creates a channel per Telegram Topic and mints its own webhooks.
		#[secret]
		discord_bot_token: String,
		/// Bot API chat id of the supergroup, e.g. `-1001234567890`.
		tg_chat_id: i64,
		/// The Discord category every mirror channel is created under. The guild is
		/// read off it at boot, so there is no second id to keep in step.
		discord_tg_category_id: String,
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
