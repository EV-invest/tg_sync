use std::time::Duration;

use color_eyre::eyre::{Result, bail};
use serde::Deserialize;

/// Discord answers a burst with 429 + `retry_after`; obeying it is the whole
/// rate-limit story. The cap only exists so an endpoint that answers 429 forever
/// crashes the pod (and replays) instead of hanging silently.
const MAX_RATE_LIMIT_RETRIES: u32 = 10;
const API: &str = "https://discord.com/api/v10";

#[derive(Debug, Deserialize)]
pub struct Channel {
	pub id: String,
	pub name: String,
	pub guild_id: Option<String>,
	pub parent_id: Option<String>,
	/// The channel description. tg-sync stores its `tg:<thread_id>` marker here.
	pub topic: Option<String>,
}

#[derive(Deserialize)]
struct Webhook {
	id: String,
	token: Option<String>,
	name: Option<String>,
}

#[derive(Deserialize)]
struct RateLimited {
	retry_after: f64,
}

pub struct Discord {
	http: reqwest::Client,
	bot_token: String,
}

impl Discord {
	pub fn try_new(bot_token: String) -> Result<Self> {
		Ok(Self {
			http: reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?,
			bot_token,
		})
	}

	/// One webhook execution. `file` rides as multipart alongside the JSON payload.
	pub async fn post(&self, webhook: &str, username: &str, content: &str, file: Option<(&str, &[u8])>) -> Result<()> {
		let payload = serde_json::json!({
			"username": username,
			"content": content,
			// Mirrored text is untrusted input from another platform — an `@everyone`
			// typed in Telegram must not page the whole Discord server.
			"allowed_mentions": { "parse": [] },
		})
		.to_string();

		// Webhook execution is the one unauthenticated call: the URL is the credential.
		self.execute(|| match file {
			Some((name, bytes)) => {
				let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(name.to_string());
				let form = reqwest::multipart::Form::new().text("payload_json", payload.clone()).part("files[0]", part);
				self.http.post(webhook).multipart(form)
			}
			None => self.http.post(webhook).header("content-type", "application/json").body(payload.clone()),
		})
		.await?;
		Ok(())
	}

	pub async fn channel(&self, id: &str) -> Result<Channel> {
		self.json(|| self.bot(self.http.get(format!("{API}/channels/{id}")))).await
	}

	pub async fn guild_channels(&self, guild_id: &str) -> Result<Vec<Channel>> {
		self.json(|| self.bot(self.http.get(format!("{API}/guilds/{guild_id}/channels")))).await
	}

	/// Type 0 = a plain text channel. Discord lowercases and dash-joins the name
	/// itself, but `slug` already did it so what lands is what we asked for.
	pub async fn create_channel(&self, guild_id: &str, name: &str, parent_id: &str, topic: &str) -> Result<Channel> {
		let body = serde_json::json!({ "name": name, "type": 0, "parent_id": parent_id, "topic": topic });
		self.json(|| self.bot(self.http.post(format!("{API}/guilds/{guild_id}/channels")).json(&body))).await
	}

	pub async fn rename_channel(&self, channel_id: &str, name: &str, topic: &str) -> Result<()> {
		let body = serde_json::json!({ "name": name, "topic": topic });
		self.execute(|| self.bot(self.http.patch(format!("{API}/channels/{channel_id}")).json(&body))).await?;
		Ok(())
	}

	/// The `tg-sync` webhook of a channel, minted on first use. Reusing one named
	/// webhook keeps re-creation idempotent across restarts — the bridge holds no
	/// state, so the channel itself is where the binding lives.
	pub async fn webhook_for(&self, channel_id: &str) -> Result<String> {
		let existing: Vec<Webhook> = self.json(|| self.bot(self.http.get(format!("{API}/channels/{channel_id}/webhooks")))).await?;
		if let Some(hook) = existing.iter().find(|w| w.name.as_deref() == Some("tg-sync") && w.token.is_some()) {
			return Ok(url_of(&hook.id, hook.token.as_deref().expect("filtered on token being present")));
		}
		let body = serde_json::json!({ "name": "tg-sync" });
		let minted: Webhook = self.json(|| self.bot(self.http.post(format!("{API}/channels/{channel_id}/webhooks")).json(&body))).await?;
		match minted.token {
			Some(token) => Ok(url_of(&minted.id, &token)),
			None => bail!("discord minted a webhook for channel {channel_id} without a token"),
		}
	}

	fn bot(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
		request.header("authorization", format!("Bot {}", self.bot_token))
	}

	async fn json<T: serde::de::DeserializeOwned, F: Fn() -> reqwest::RequestBuilder>(&self, build: F) -> Result<T> {
		let text = self.execute(build).await?;
		serde_json::from_str(&text).map_err(|e| color_eyre::eyre::eyre!("discord returned an unparseable body: {e}: {text}"))
	}

	/// The one place 429 is handled, so every call — not just the message post —
	/// obeys `retry_after` instead of dropping work on a burst. The request is
	/// rebuilt per attempt because a multipart body is consumed by `send`.
	async fn execute<F: Fn() -> reqwest::RequestBuilder>(&self, build: F) -> Result<String> {
		for attempt in 0..=MAX_RATE_LIMIT_RETRIES {
			let response = build().send().await?;
			let status = response.status();
			let text = response.text().await?;
			if status != reqwest::StatusCode::TOO_MANY_REQUESTS {
				if !status.is_success() {
					bail!("discord rejected the request (status {status}): {text}");
				}
				return Ok(text);
			}
			// The status already established we were throttled; an unparseable body
			// only costs us the exact delay, and 1s is Discord's own bucket floor.
			let retry_after = serde_json::from_str::<RateLimited>(&text).map(|r| r.retry_after).unwrap_or(1.0);
			tracing::warn!(attempt, retry_after, "discord rate limited the request");
			tokio::time::sleep(Duration::from_secs_f64(retry_after)).await;
		}
		bail!("discord kept rate limiting after {MAX_RATE_LIMIT_RETRIES} retries")
	}
}

fn url_of(id: &str, token: &str) -> String {
	format!("https://discord.com/api/webhooks/{id}/{token}")
}
