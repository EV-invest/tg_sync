# tg_sync

One-way bridge mirroring the EV Telegram supergroup's forum **Topics** into Discord
channels, so Discord is a superset of the team's discussion rather than half of it.

Long-polls `getUpdates` and re-posts each message through a Discord **webhook** per
mirror channel — webhooks take a per-message `username` override, so a mirrored post
renders under its original Telegram author. No bot gateway, no database, no PVC.

## Guarantees

- **Nothing is silently dropped.** Unmapped topics, service messages and files over
  the relay cap all still land — the last two as a stub carrying a `source` deep link.
- **At-least-once.** Telegram deletes an update server-side only once the *next*
  `getUpdates` confirms it, so the offset advances after a successful Discord post.
  A crash replays the unconfirmed tail; there is nothing to persist.
- **Bounded media.** `getFile` refuses over 20 MB and Discord's free tier refuses
  over 10 MB, so `TG_MEDIA_MAX_BYTES` (default 8 MB) decides inline vs. stub.

## Configuration (env only — `ev::settings!`, no config file)

| var | source | notes |
| --- | --- | --- |
| `TELEGRAM_BOT_TOKEN` | sops → `kubernetes-tg-sync` | secret |
| `TG_CHAT_ID` | gitops `flake.nix` | `-100…` supergroup id; other chats are ignored |
| `TG_TOPIC_MAP` | gitops `flake.nix` | `"1:general,12:dev"` — General topic is thread id 1 |
| `TG_MEDIA_MAX_BYTES` | gitops `flake.nix` | inline relay ceiling |
| `DISCORD_WEBHOOK_TG_<SLUG>` | rpi5 host | one per slug in the map |
| `DISCORD_WEBHOOK_TG_UNMAPPED` | rpi5 host | catch-all; a new Topic announces itself here |

A slug in `TG_TOPIC_MAP` without its webhook is a **boot failure**, not a runtime
surprise.

## Local run

```sh
nix develop
cp .env.example .env   # fill in the token + webhooks
cargo r
```

Deploys through the gitops container contract: tag `vX.Y.Z` → GHCR → Flux image
automation. See `EV-invest/gitops`.
