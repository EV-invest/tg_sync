#![feature(default_field_values)]
//! One-way bridge: the EV Telegram supergroup → Discord.
//!
//! Long-polls `getUpdates` and re-posts every message through a Discord webhook,
//! one webhook per mirror channel, so each post renders under its original
//! Telegram author's name. No bot gateway, no database, no PVC.
//!
//! **No declarative topic map.** A forum Topic gets its Discord channel the first
//! time it says anything — created under the configured category, named after the
//! Topic, renamed when the Topic is. See [`channels`].
//!
//! **Offset durability without storage.** Telegram drops an update server-side
//! only once the next `getUpdates` confirms it, so the offset advances *after* a
//! successful Discord post. A crash therefore replays the unconfirmed tail —
//! at-least-once, bounded duplicates, never loss, and nothing to persist.

mod channels;
mod config;
mod discord;
mod render;
mod telegram;

use channels::{Channels, GENERAL_THREAD_ID};
use color_eyre::eyre::{Context, Result};
use config::Config;
use discord::Discord;
use telegram::Telegram;

fn main() -> Result<()> {
	color_eyre::install()?;
	dotenvy::dotenv().ok();

	// Exits 78 (EX_CONFIG) on a bad environment: a restart cannot help.
	let config = ev::settings::or_exit(Config::from_env());
	let _otel_guard = init_tracing(&config.app_env);

	tokio::runtime::Builder::new_multi_thread()
		.enable_all()
		.build()
		.context("failed to build tokio runtime")?
		.block_on(run(config))
}

async fn run(config: Config) -> Result<()> {
	let telegram = Telegram::try_new(config.telegram_bot_token.clone(), config.tg_poll_timeout_secs)?;
	// Resolving the category here means a bad id, a revoked Discord token or a
	// missing permission fails the boot instead of the first message.
	let channels = Channels::try_new(Discord::try_new(config.discord_bot_token.clone())?, config.discord_tg_category_id.clone())
		.await
		.context("failed to open the Discord mirror category")?;

	let listener = tokio::net::TcpListener::bind(config.bind).await.with_context(|| format!("failed to bind {}", config.bind))?;
	tracing::info!(bind = %config.bind, chat = config.tg_chat_id, "tg-sync mirroring");
	let health = axum::serve(listener, axum::Router::new().route("/health", axum::routing::get(|| async { "ok" })));

	// Whichever arm finishes ends the process: on SIGTERM the unconfirmed tail is
	// simply replayed by the next pod, which is the same guarantee a crash gets.
	tokio::select! {
		result = health => result.context("health server error")?,
		result = mirror(&telegram, channels, &config) => result?,
		_ = await_signal() => tracing::info!("shutdown signal received"),
	}
	Ok(())
}

async fn mirror(telegram: &Telegram, mut channels: Channels, config: &Config) -> Result<()> {
	// 0 asks for everything still pending, which after a crash is exactly the
	// tail that never reached Discord.
	let mut offset = 0;
	// Ends by returning an error (crashing the pod, which replays the unconfirmed
	// tail) or by losing the `select!` in `run` to the shutdown signal.
	//LOOP: unbounded by design — the poll IS the process's lifetime.
	loop {
		for update in telegram.get_updates(offset).await? {
			let edited = update.message.is_none();
			match update.message.as_ref().or(update.edited_message.as_ref()) {
				// The bot only mirrors the one group it was configured for; being added
				// to another chat must not start creating channels for it.
				Some(message) if message.chat.id != config.tg_chat_id => {
					tracing::warn!(chat = message.chat.id, "message from an unconfigured chat — not mirrored")
				}
				Some(message) => relay(telegram, &mut channels, config, message, edited).await?,
				// Structural, not transient: crashing would replay this update forever.
				// The error reaches Discord through the `tg-sync` alert front instead.
				None => tracing::error!(update_id = update.update_id, "update carries neither message nor edited_message — skipping"),
			}
			offset = update.update_id + 1;
		}
	}
}

/// Every failure here propagates: the offset is not advanced, so k8s restarts the
/// pod and Telegram hands the same message back. Duplicates are bounded; loss is not.
///
// ponytail: a message Discord rejects *permanently* (a 400 no retry can fix)
// crashloops forever instead of advancing — the deliberate price of "never drop".
// The tg-sync alert front is what surfaces it; add a poison-message quarantine
// channel only once one actually shows up.
async fn relay(telegram: &Telegram, channels: &mut Channels, config: &Config, message: &telegram::Message, edited: bool) -> Result<()> {
	let post = render::render(message, edited, config.tg_media_max_bytes);
	assert!(!post.chunks.is_empty(), "render always emits the source subtext line");

	let bytes = match &post.file {
		Some(file) => Some((
			file.name.as_str(),
			telegram.download(&file.file_id).await.with_context(|| format!("failed to download {}", file.name))?,
		)),
		None => None,
	};

	let thread = message.message_thread_id.unwrap_or(GENERAL_THREAD_ID);
	let webhook = channels.webhook(thread, render::topic_name(message)).await?.to_string();

	// The attachment rides the last chunk so it lands under the full text.
	let last = post.chunks.len() - 1;
	for (i, chunk) in post.chunks.iter().enumerate() {
		let file = if i == last { bytes.as_ref().map(|(name, data)| (*name, data.as_slice())) } else { None };
		channels.discord().post(&webhook, &post.username, chunk, file).await.context("failed to post to Discord")?;
	}
	Ok(())
}

async fn await_signal() {
	use tokio::signal::unix::{SignalKind, signal};
	let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler installs on every unix k8s runs on");
	tokio::select! {
		result = tokio::signal::ctrl_c() => result.expect("ctrl-c handler installs on every unix k8s runs on"),
		_ = term.recv() => {},
	}
}

fn init_tracing(environment: &str) -> Option<ev::otel::Telemetry> {
	use tracing_subscriber::{EnvFilter, fmt, prelude::*};

	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tg_sync=debug"));
	let (otel_guard, otel_layers) = ev::otel::telemetry(&ev::otel::Config {
		environment: environment.to_string(),
		traces_sample_rate: ev::otel::Config::traces_sample_rate_for(environment),
	})
	.unzip();
	tracing_subscriber::registry().with(filter).with(fmt::layer()).with(otel_layers).init();
	otel_guard
}
