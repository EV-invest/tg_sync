use std::time::Duration;

use color_eyre::eyre::{Result, bail};
use serde::Deserialize;

/// Discord answers a burst with 429 + `retry_after`; obeying it is the whole
/// rate-limit story. The cap only exists so a webhook that answers 429 forever
/// crashes the pod (and replays) instead of hanging silently.
const MAX_RATE_LIMIT_RETRIES: u32 = 10;

#[derive(Deserialize)]
struct RateLimited {
	retry_after: f64,
}

pub struct Discord {
	http: reqwest::Client,
}

impl Discord {
	pub fn new() -> Result<Self> {
		Ok(Self {
			http: reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?,
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

		for attempt in 0..=MAX_RATE_LIMIT_RETRIES {
			// The multipart form is consumed by `send`, so it is rebuilt per attempt.
			let request = match file {
				Some((name, bytes)) => {
					let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(name.to_string());
					let form = reqwest::multipart::Form::new().text("payload_json", payload.clone()).part("files[0]", part);
					self.http.post(webhook).multipart(form)
				}
				None => self.http.post(webhook).header("content-type", "application/json").body(payload.clone()),
			};

			let response = request.send().await?;
			let status = response.status();
			if status != reqwest::StatusCode::TOO_MANY_REQUESTS {
				if !status.is_success() {
					bail!("discord webhook rejected the post (status {status}): {}", response.text().await.unwrap_or_default());
				}
				return Ok(());
			}

			let text = response.text().await?;
			// The status already established we were throttled; an unparseable body
			// only costs us the exact delay, and 1s is Discord's own bucket floor.
			let retry_after = serde_json::from_str::<RateLimited>(&text).map(|r| r.retry_after).unwrap_or(1.0);
			tracing::warn!(attempt, retry_after, "discord rate limited the webhook");
			tokio::time::sleep(Duration::from_secs_f64(retry_after)).await;
		}
		bail!("discord kept rate limiting the webhook after {MAX_RATE_LIMIT_RETRIES} retries")
	}
}
