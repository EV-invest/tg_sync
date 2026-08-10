use std::time::Duration;

use color_eyre::eyre::{Result, bail, eyre};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Update {
	pub update_id: i64,
	pub message: Option<Message>,
	pub edited_message: Option<Message>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
	pub message_id: i64,
	pub message_thread_id: Option<i64>,
	pub chat: Chat,
	pub from: Option<User>,
	pub text: Option<String>,
	pub caption: Option<String>,
	pub reply_to_message: Option<Box<Message>>,
	pub forum_topic_created: Option<ForumTopicCreated>,
	pub forum_topic_edited: Option<ForumTopicEdited>,
	/// Same photo in ascending sizes; the relay picks the largest that fits.
	pub photo: Option<Vec<PhotoSize>>,
	pub video: Option<FileMeta>,
	pub document: Option<FileMeta>,
	pub audio: Option<FileMeta>,
	pub voice: Option<FileMeta>,
	pub video_note: Option<FileMeta>,
	pub animation: Option<FileMeta>,
	pub sticker: Option<FileMeta>,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
	pub id: i64,
}

#[derive(Debug, Deserialize)]
pub struct User {
	pub first_name: String,
	pub last_name: Option<String>,
}
impl User {
	/// Display name for the Discord webhook identity override.
	pub fn display(&self) -> String {
		match &self.last_name {
			Some(last) => format!("{} {last}", self.first_name),
			None => self.first_name.clone(),
		}
	}
}

#[derive(Debug, Deserialize)]
pub struct ForumTopicCreated {
	pub name: String,
}

/// A rename carries only what changed, so `name` is absent when the edit touched
/// just the icon.
#[derive(Debug, Deserialize)]
pub struct ForumTopicEdited {
	pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PhotoSize {
	pub file_id: String,
	pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct FileMeta {
	pub file_id: String,
	pub file_name: Option<String>,
	pub mime_type: Option<String>,
	pub file_size: Option<u64>,
	/// Stickers only.
	pub emoji: Option<String>,
}

pub struct Telegram {
	http: reqwest::Client,
	token: String,
	poll_timeout: u64,
}
impl Telegram {
	pub fn try_new(token: String, poll_timeout: u64) -> Result<Self> {
		// The long poll holds the connection for `poll_timeout`; the client budget
		// must clear it or every idle poll would look like a network failure.
		let http = reqwest::Client::builder().timeout(Duration::from_secs(poll_timeout + 35)).build()?;
		Ok(Self { http, token, poll_timeout })
	}

	/// Long-poll. `offset` doubles as the acknowledgement: Telegram drops every
	/// update below it server-side, which is why the caller must only advance it
	/// past a message that already reached Discord.
	pub async fn get_updates(&self, offset: i64) -> Result<Vec<Update>> {
		self.call(
			"getUpdates",
			&serde_json::json!({
				"offset": offset,
				"timeout": self.poll_timeout,
				"allowed_updates": ["message", "edited_message"],
			}),
		)
		.await
	}

	/// `getFile` + fetch. Refuses over 20 MB on Telegram's side, which is why the
	/// caller checks `file_size` against its own (lower) cap first.
	pub async fn download(&self, file_id: &str) -> Result<Vec<u8>> {
		let file: File = self.call("getFile", &serde_json::json!({ "file_id": file_id })).await?;
		let path = file.file_path.ok_or_else(|| eyre!("getFile returned no file_path for {file_id}"))?;
		let bytes = self
			.http
			.get(format!("https://api.telegram.org/file/bot{}/{path}", self.token))
			.send()
			.await?
			.error_for_status()?
			.bytes()
			.await?;
		Ok(bytes.to_vec())
	}

	/// Reads the body before checking the status: a Bot API failure is an HTTP 4xx
	/// whose `description` is the only thing that says *why* (a revoked token, a
	/// second poller holding the same offset, a file over 20 MB).
	async fn call<T: serde::de::DeserializeOwned>(&self, method: &str, body: &serde_json::Value) -> Result<T> {
		let response = self.http.post(format!("https://api.telegram.org/bot{}/{method}", self.token)).json(body).send().await?;
		let status = response.status();
		let text = response.text().await?;
		let parsed: ApiResponse<T> = serde_json::from_str(&text).map_err(|e| eyre!("telegram {method} returned an unparseable body (status {status}): {e}: {text}"))?;
		if !parsed.ok {
			bail!("telegram {method} failed (status {status}): {}", parsed.description.unwrap_or_default());
		}
		parsed.result.ok_or_else(|| eyre!("telegram {method} returned ok without a result"))
	}
}

#[derive(Deserialize)]
struct ApiResponse<T> {
	ok: bool,
	result: Option<T>,
	description: Option<String>,
}

#[derive(Deserialize)]
struct File {
	file_path: Option<String>,
}
