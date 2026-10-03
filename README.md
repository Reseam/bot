<p align="center">
  <img src="https://reseam.app/logo.svg" alt="Reseam logo" width="96">
</p>

<h1 align="center">Reseam Bot</h1>

The Discord bot for the Reseam server. It has an AI agent that answers questions, reads and summarizes channels, and works with Reseam's repositories and issues. It also has moderation commands.

How to use it in Discord is in [GUIDE.md](GUIDE.md). This README covers running it.

## Access

- **Owners** (`access.owner_ids`) and **team roles** (`access.team_role_ids`) get the full agent.
- **Member roles** (`access.member_role_ids`, the AI Access role) get chat, Discord, and file tools only: no web, repositories, forge APIs, MCP, Python, or JavaScript.
- Moderation commands follow normal Discord permissions.

Mention the bot or reply to it to start a run, and reply to its answer to continue. Actions with an effect outside the conversation ask the person who started the run to approve first: moderation, messages to other channels, HTTP requests other than GET and HEAD, and MCP tools listed under `approve`.

## Commands

| Command | Who | What |
|---|---|---|
| `/ask prompt [file]` | team | Ask the agent something |
| `/summarize [channel] [messages] [since]` | team | Summarize a channel in that channel. `since` reads back until the cutoff |
| `Summarize from here` (message menu) | team | Summarize from a message to now |
| `Ask about this` (message menu) | team | Ask a question about a message |
| `Create issue` (message menu) | team | Draft an issue from a message and create it after approval |
| `/remind set`, `/remind list`, `/remind cancel` | team | Ping yourself in the channel after a delay, list your reminders, or cancel one |
| `/mcp status` | team | MCP server status and tools |
| `/mcp reconnect server` | owners | Reconnect an MCP server |
| `/personality` | owners | Write or clear extra instructions for the bot's tone and style |
| `/warn`, `/timeout`, `/untimeout` | Moderate Members | |
| `/kick` | Kick Members | |
| `/ban`, `/unban` | Ban Members | `/ban` works on users who already left and takes an optional duration |
| `/purge` | Manage Messages | Delete up to 100 matching messages from the last 14 days |
| `/slowmode`, `/lock`, `/unlock` | Manage Channels | `/lock` saves the channel's permissions and `/unlock` restores them |
| `/modlog [channel]` | Manage Server | Set or show the moderation log channel |

## Sandbox

The agent has one tool, `bash`. It runs in [just-bash](https://github.com/vercel-labs/just-bash), an emulated shell with its own filesystem, inside a Node service the bot starts (`sandbox/`). The shell can't see the bot's files, environment, or network.

- `/workspace` belongs to the conversation. `/repos/<host>/<owner>/<name>` shows cloned repositories; edits there stay in memory. Both are deleted after 8 hours without use.
- Built in: coreutils, `rg`, `jq`, `yq`, `sqlite3`, `python3` (standard library), `js-exec`, and `curl`. `curl` reaches public hosts only; the bot adds forge tokens for configured forge APIs.
- Bridge commands run in the bot with the permissions of the person who asked:
  - `discord`: read and send messages, attachments, threads, pins, polls, reminders, the audit log, and `discord mod` for moderation.
  - `repo clone URL [--ref REF] [--history]`: clone or update an HTTPS repository.
  - `mcp`: call tools from servers configured under `[mcp.*]`.
  - `upload`: send a file's exact bytes to a URL (team only).
  - `archive`: create, list, and extract ZIP files.
  - `view FILE`: attach an image to the answer.

Run any bridge command with `--help` for its flags and limits.

## Configuration

`config.toml` is committed and only references environment variables with `${NAME}` or `${NAME:-default}`. A missing variable without a default stops startup with an error naming it. Copy `.env.example` to `.env` for local runs.

| Variable | Purpose |
|---|---|
| `DISCORD_TOKEN` | Bot token |
| `DISCORD_GUILD_ID` | Server the slash commands register to |
| `DISCORD_OWNER_ID`, `DISCORD_TEAM_ROLE_ID`, `DISCORD_AI_ROLE_ID` | Who can use the AI |
| `DATA_DIR` | SQLite database, sandbox workspaces, cloned repositories |
| `LLM_BASE_URL`, `LLM_API_KEY`, `LLM_MODEL` | Any OpenAI-compatible Chat Completions endpoint |
| `FORGEJO_URL`, `FORGEJO_TOKEN`, `GITHUB_TOKEN` | Forge access; without a token, only public data works |
| `EXA_API_KEY` | Exa MCP server |
| `SANDBOX_ENTRY` | Path to the built sandbox service (default `sandbox/dist/main.js`) |

Notable settings in `config.toml`:

- `[llm]` `context_window` and `max_output_tokens` must match the model. Set `vision = false` for models without image input. `extra_body` is merged into every request, e.g. `extra_body = { reasoning = { effort = "medium" } }`.
- `[agent]` `max_turns` (default 100; the last step has no tools and must answer), `history_messages` (0 to 100, default 0), `conversation_retention_days` (default 30), `compact_at_tokens` (default 500000), `compaction_reserve_tokens`, `keep_recent_tokens`.
- `[forges.<name>]` `kind` (`github` or `forgejo`), `url`, `token`, optional `default_repo`.
- `[mcp.<name>]` either `url` (streamable HTTP, optional `headers`) or `command` with `args` and `env` (stdio). Optional `tools` allowlist, `approve` list, and `timeout_secs`.

## Discord application

- Privileged intents: **Message Content** and **Server Members**.
- Invite with the `bot` and `applications.commands` scopes and these permissions: View Channels, Send Messages, Send Messages in Threads, Create Public Threads, Embed Links, Attach Files, Read Message History, Add Reactions, Use External Emojis, Manage Messages, Pin Messages, Manage Threads, Manage Channels, Manage Roles, Kick Members, Ban Members, Moderate Members (permission integer `2253226011651158`).
- Moderating a member requires the bot's highest role to be above the member's highest role. The bot reads a private thread only when it has been added to it and the person asking is a member.

## Run locally

```sh
cp .env.example .env   # fill it in
npm --prefix sandbox ci && npm --prefix sandbox run build
cargo run
```

`cargo test` runs the unit and mock-server tests. `cargo test -- --ignored` also runs live tests against the configured LLM and Exa.

## Deploy

Pushing to `main` builds the bot and the sandbox on the Forgejo runner and uploads them, with `config.toml`, as `reseam-bot-linux-x64.tar.gz` to the rolling `latest` release. CI then calls the Dokploy deploy webhook. Dokploy builds the `Dockerfile`, which unpacks that release on `node:24-trixie-slim`. Production environment variables live on the `bot` application in Dokploy, and the `bot-data` volume is mounted at `/var/lib/reseam-bot`.
