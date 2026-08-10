# tg_sync

One-way bridge mirroring the EV Telegram supergroup's forum **Topics** into Discord
channels, so Discord is a superset of the team's discussion rather than half of it.

Long-polls `getUpdates` and re-posts each message through a Discord **webhook** per
mirror channel — webhooks take a per-message `username` override, so a mirrored post
renders under its original Telegram author. No gateway connection, no database, no PVC.

**There is no topic map.** A Topic gets its Discord channel the first time it says
anything: created under the configured category, named after the Topic, renamed when
the Topic is. The binding lives in Discord — each mirror channel carries a
`tg:<thread_id>` marker in its description — so lookup survives a rename on either
side and the bridge stays stateless across restarts.

The Bot API cannot list a forum's Topics, so a name is harvested from whatever an
update reveals (`forum_topic_created`, `forum_topic_edited`, or the
`reply_to_message` Telegram attaches to ordinary Topic messages). A Topic that has
not yet named itself is mirrored as `#topic-<id>` and renamed the moment it does.

## Guarantees

- **Nothing is silently dropped.** Unmapped topics, service messages and files over
  the relay cap all still land — the last two as a stub carrying a `source` deep link.
- **At-least-once.** Telegram deletes an update server-side only once the *next*
  `getUpdates` confirms it, so the offset advances after a successful Discord post.
  A crash replays the unconfirmed tail; there is nothing to persist.
- **Self-configuring.** A new Topic needs no deploy — it creates its own channel.
- **Bounded media.** `getFile` refuses over 20 MB and Discord's free tier refuses
  over 10 MB, so `TG_MEDIA_MAX_BYTES` (default 8 MB) decides inline vs. stub.

## Configuration (env only — `ev::settings!`, no config file)

| var | source | notes |
| --- | --- | --- |
| `TELEGRAM_BOT_TOKEN` | sops → `kubernetes-tg-sync` | secret; privacy mode must be OFF |
| `DISCORD_BOT_TOKEN` | sops → `kubernetes-tg-sync` | secret; Manage Channels + Manage Webhooks |
| `TG_CHAT_ID` | gitops `flake.nix` | `-100…` supergroup id; other chats are ignored |
| `DISCORD_TG_CATEGORY_ID` | gitops `flake.nix` | mirror channels are created under it |
| `TG_MEDIA_MAX_BYTES` | gitops `flake.nix` | inline relay ceiling |

A bad category id, a revoked Discord token or a missing permission is a **boot
failure**, not a surprise on the first message.

## Local run

```sh
nix develop
cp .env.example .env   # fill in the two bot tokens
cargo r
```

Deploys through the gitops container contract: tag `vX.Y.Z` → GHCR → Flux image
automation. See `EV-invest/gitops`.
