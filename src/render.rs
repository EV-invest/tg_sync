//! Telegram [`Message`] → the Discord post that mirrors it.
//!
//! Pure: no I/O, no clock. Whether a file rides inline or as a stub is decided
//! here from `file_size`, so the caller only has to fetch what came back as a
//! [`PendingFile`].

use crate::telegram::{FileMeta, Message};

/// Discord's `content` ceiling. Telegram allows 4096, so long messages split
/// across sequential posts rather than being truncated.
pub const CONTENT_LIMIT: usize = 2000;
/// Discord rejects a webhook `username` over 80 characters.
const USERNAME_LIMIT: usize = 80;
const REPLY_SNIPPET: usize = 40;

pub struct Post {
	pub username: String,
	/// At least one, each within [`CONTENT_LIMIT`]. Posted in order.
	pub chunks: Vec<String>,
	pub file: Option<PendingFile>,
}

pub struct PendingFile {
	pub file_id: String,
	pub name: String,
}

/// The Topic's name, if this update happens to reveal it.
///
/// The Bot API cannot list a forum's Topics, so this is the only source there is.
/// It is reliable in practice: Telegram sets `reply_to_message` to the Topic's
/// creation message for every message in the Topic that is not a reply to some
/// other message, so any conversation names its Topic within a message or two.
pub fn topic_name(message: &Message) -> Option<&str> {
	fn named(m: &Message) -> Option<&str> {
		m.forum_topic_edited
			.as_ref()
			.and_then(|e| e.name.as_deref())
			.or(m.forum_topic_created.as_ref().map(|c| c.name.as_str()))
	}
	named(message).or_else(|| message.reply_to_message.as_deref().and_then(named))
}

pub fn render(message: &Message, edited: bool, max_bytes: u64) -> Post {
	let source = source_link(message.chat.id, message.message_thread_id, message.message_id);

	let mut head = String::new();
	// A forum Topic's first message carries the topic-creation service message as
	// its `reply_to_message`; quoting that would prefix every topic with itself.
	if let Some(reply) = message.reply_to_message.as_deref().filter(|r| r.forum_topic_created.is_none()) {
		let who = reply.from.as_ref().map_or_else(|| "someone".to_string(), |u| u.display());
		head.push_str(&format!("-# ↳ replying to {who}: \"{}\"\n", snippet(reply)));
	}

	// The Topic lifecycle reads as the channel's own first line, so the mirror
	// explains where it came from without anyone consulting Telegram.
	let mut body = match (&message.forum_topic_created, &message.forum_topic_edited) {
		(Some(topic), _) => format!("🆕 mirroring the Telegram topic **{}**", topic.name),
		(_, Some(edit)) => match &edit.name {
			Some(name) => format!("✏️ the Telegram topic is now called **{name}**"),
			None => String::new(),
		},
		_ => message.text.clone().or_else(|| message.caption.clone()).unwrap_or_default(),
	};

	// A sticker's only text is its emoji; without it the post is a bare .webp
	// (or a .tgs Discord cannot render at all).
	if body.is_empty() {
		if let Some(emoji) = message.sticker.as_ref().and_then(|s| s.emoji.as_deref()) {
			body.push_str(emoji);
		}
	}

	let mut file = None;
	match pick_media(message, max_bytes) {
		Media::None => {}
		Media::Inline { file_id, name } => file = Some(PendingFile { file_id: file_id.to_string(), name }),
		Media::Over { name, kind, size } => {
			if !body.is_empty() {
				body.push('\n');
			}
			body.push_str(&format!("📎 **{name}** · {kind} · {} — over the {} relay cap", human_size(size), human_size(Some(max_bytes))));
		}
	}

	// Nothing rendered at all (a join/leave/pin service message): still mirror it,
	// because a silent drop is the desync this bridge exists to kill.
	if body.is_empty() && file.is_none() {
		body.push_str("-# (service message with no relayable content)");
	}

	let mut tail = format!("-# [source]({source})");
	if edited {
		tail.push_str(" · (edited)");
	}

	let mut chunks = split(&(head + &body), CONTENT_LIMIT);
	match chunks.last_mut() {
		Some(last) if last.chars().count() + 1 + tail.chars().count() <= CONTENT_LIMIT => {
			last.push('\n');
			last.push_str(&tail);
		}
		_ => chunks.push(tail),
	}

	Post {
		username: username(message),
		chunks,
		file,
	}
}
/// What the message carries besides text, once measured against the relay cap.
enum Media<'a> {
	None,
	Inline {
		file_id: &'a str,
		name: String,
	},
	/// Over the cap, or of unknown size — either way the bytes never move.
	Over {
		name: String,
		kind: &'static str,
		size: Option<u64>,
	},
}

/// `t.me/c/<channel>/<thread>/<id>` — the private-supergroup form documented at
/// core.telegram.org/api/links. `<channel>` is the Bot API chat id with its
/// `-100` prefix stripped; the thread segment is absent for the General topic.
/// Resolves only for members, and only inside a Telegram client.
fn source_link(chat_id: i64, thread_id: Option<i64>, message_id: i64) -> String {
	let channel = chat_id.to_string();
	let channel = channel.strip_prefix("-100").unwrap_or(&channel);
	match thread_id {
		Some(thread) => format!("https://t.me/c/{channel}/{thread}/{message_id}"),
		None => format!("https://t.me/c/{channel}/{message_id}"),
	}
}

fn username(message: &Message) -> String {
	let mut name = message.from.as_ref().map_or_else(|| "Telegram".to_string(), |u| u.display());
	if name.chars().count() > USERNAME_LIMIT {
		name = name.chars().take(USERNAME_LIMIT).collect();
	}
	name
}

fn snippet(message: &Message) -> String {
	let text = message.text.as_deref().or(message.caption.as_deref()).unwrap_or("<media>").replace('\n', " ");
	if text.chars().count() <= REPLY_SNIPPET {
		return text;
	}
	text.chars().take(REPLY_SNIPPET).collect::<String>() + "…"
}

fn pick_media(message: &Message, max_bytes: u64) -> Media<'_> {
	// Photos arrive as one entry per size; take the biggest that fits, and if none
	// does, report the biggest so the stub names the real weight.
	if let Some(sizes) = message.photo.as_ref().filter(|s| !s.is_empty()) {
		let fits = sizes.iter().filter(|s| s.file_size.is_some_and(|n| n <= max_bytes)).next_back();
		return match fits {
			Some(size) => Media::Inline {
				file_id: &size.file_id,
				name: "photo.jpg".to_string(),
			},
			None => Media::Over {
				name: "photo.jpg".to_string(),
				kind: "photo",
				size: sizes.last().and_then(|s| s.file_size),
			},
		};
	}
	let slots: [(&Option<FileMeta>, &'static str); 7] = [
		(&message.video, "video"),
		(&message.animation, "animation"),
		(&message.document, "document"),
		(&message.audio, "audio"),
		(&message.voice, "voice"),
		(&message.video_note, "video note"),
		(&message.sticker, "sticker"),
	];
	let Some((meta, kind)) = slots.into_iter().find_map(|(slot, kind)| slot.as_ref().map(|m| (m, kind))) else {
		return Media::None;
	};

	let name = file_name(meta, kind);
	// An absent `file_size` is treated as oversized: Telegram omits it only for
	// things it will not size for us, and guessing risks a download that cannot land.
	match meta.file_size {
		Some(size) if size <= max_bytes => Media::Inline { file_id: &meta.file_id, name },
		size => Media::Over { name, kind, size },
	}
}

fn file_name(meta: &FileMeta, kind: &str) -> String {
	if let Some(name) = &meta.file_name {
		return name.clone();
	}
	let ext = meta.mime_type.as_deref().and_then(|m| m.split_once('/')).map_or(String::new(), |(_, sub)| format!(".{sub}"));
	format!("{}{ext}", kind.replace(' ', "_"))
}

fn human_size(bytes: Option<u64>) -> String {
	match bytes {
		Some(n) => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
		None => "unknown size".to_string(),
	}
}

/// Greedy split on the last newline (else the last space) inside each window, so
/// a long paste breaks between lines instead of mid-word. Empty input yields no
/// chunks — the caller supplies the subtext line.
fn split(content: &str, limit: usize) -> Vec<String> {
	let mut chunks = Vec::new();
	let mut rest = content;
	while !rest.is_empty() {
		if rest.chars().count() <= limit {
			chunks.push(rest.to_string());
			break;
		}
		let end = rest.char_indices().nth(limit).map_or(rest.len(), |(i, _)| i);
		let window = &rest[..end];
		let cut = window.rfind('\n').or_else(|| window.rfind(' ')).map_or(end, |i| i + 1);
		let chunk = rest[..cut].trim_end();
		// A window that is all whitespace leaves nothing to post; Discord rejects
		// an empty `content`, so drop the seam rather than the message.
		if !chunk.is_empty() {
			chunks.push(chunk.to_string());
		}
		rest = &rest[cut..];
	}
	chunks
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_general_topic_link_has_no_thread_segment() {
		assert_eq!(source_link(-1001234567890, Some(12), 4567), "https://t.me/c/1234567890/12/4567");
		assert_eq!(source_link(-1001234567890, None, 4567), "https://t.me/c/1234567890/4567");
	}

	#[test]
	fn splitting_never_exceeds_the_limit_and_loses_nothing() {
		let content = "line one\n".repeat(500);
		let chunks = split(&content, CONTENT_LIMIT);
		assert!(chunks.len() > 1, "4500 chars must split");
		assert!(chunks.iter().all(|c| c.chars().count() <= CONTENT_LIMIT));
		// Only the whitespace at the seams is dropped.
		assert_eq!(chunks.join("").replace(['\n', ' '], ""), content.replace(['\n', ' '], ""));
	}

	#[test]
	fn splitting_a_word_longer_than_the_window_still_terminates() {
		let content = "x".repeat(CONTENT_LIMIT * 2 + 7);
		let chunks = split(&content, CONTENT_LIMIT);
		assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), content.len());
		assert!(chunks.iter().all(|c| c.chars().count() <= CONTENT_LIMIT));
	}
}
