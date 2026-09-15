# Reseam Bot

Discord bot for the Reseam team with a built-in AI agent, plus moderation commands.

## Using it

Only owners (`access.owner_ids`) and members with an access role (`access.role_ids`, the Team and AI Access roles) can use the AI. Moderation commands use normal Discord permissions instead.

- **Mention the bot** or **reply to one of its messages**. It reads the message you replied to and any attached images, PDFs, DOCX, or text files, then streams its answer. It reads more of the channel on demand, and `agent.history_messages` (0 to 100, default 0) adds that many earlier messages up front. Press **Stop** to end a run.
- **Reply to its answer** to continue the same conversation. Earlier turns and command output are kept in SQLite. Conversations are summarized once they pass `agent.compact_at_tokens`.
- **Reply to its message while it is still working** to steer the run. The bot reacts with 👀 when your message is picked up.
- **Another member replying mid-run** is queued. The bot reacts with ⏳ and starts their turn once the current run finishes.
- **Approvals.** Moderation, messages to other channels, HTTP requests other than GET and HEAD, and MCP tools listed under `approve` post an Approve / Approve for this run / Deny prompt, deleted once it is answered. Only the person who started the run can answer it. "Approve for this run" covers the same command against the same target, such as every `PATCH` to one host.

### Commands

| Command | Who | What |
|---|---|---|
| `/ask prompt [file]` | team | Ask the agent something |
| `/summarize [channel] [messages] [since]` | team | Summarize a channel in that channel. `since` reads back until the cutoff |
| `Summarize from here` (message menu) | team | Summarize from a message to now |
| `Ask about this` (message menu) | team | Ask a question about a message |
| `Create issue` (message menu) | team | Draft an issue from a message and create it after approval |
| `/mcp status` | team | MCP server status and tools |
| `/mcp reconnect server` | owners | Reconnect an MCP server |
| `/personality` | owners | Write or clear extra instructions for the bot's tone and style |
| `/warn`, `/timeout`, `/untimeout` | Moderate Members | |
| `/kick` | Kick Members | |
| `/ban`, `/unban` | Ban Members | `/ban` works on users who already left and takes an optional duration |
| `/purge` | Manage Messages | Delete up to 100 matching messages from the last 14 days |
| `/slowmode`, `/lock`, `/unlock` | Manage Channels | `/lock` saves the channel's permissions and `/unlock` restores them |
| `/modlog [channel]` | Manage Server | Set or show the moderation log channel |

### The agent's sandbox

The agent has one tool, `bash`. It runs in [just-bash](https://github.com/vercel-labs/just-bash), an emulated shell with its own filesystem, inside a Node service the bot starts (`sandbox/`). The shell cannot see the bot's files, environment, or network.

- `/workspace` is writable and belongs to the conversation. `/repos/<host>/<owner>/<name>` shows cloned repositories; edits there stay in memory.
- Workspaces and clones unused for 8 hours are deleted.
- Built-in tools include coreutils, `rg`, `jq`, `yq`, `sqlite3`, `python3` (standard library), `js-exec`, and `curl`.
- `curl` reaches public hosts only. The bot adds forge tokens for requests to configured forge APIs, and asks for approval before any request that is not GET or HEAD.

Bridge commands run in the bot with the permissions of the person who asked:

- `discord`: messages, attachments, channels, members, server info, send, react, threads, pins, and `discord mod` for moderation.
- `repo clone URL [--ref REF] [--history]`: clone or update an HTTPS repository.
- `mcp list`, `mcp SERVER TOOL --help`, `mcp SERVER TOOL key=value`: call tools from servers configured under `[mcp.*]`.
- `view FILE`: attach an image from the sandbox to the result.

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
- `[agent]` `max_turns`, `history_messages` (0 to 100, default 0), `conversation_retention_days` (default 30), `compact_at_tokens` (default 500000), `compaction_reserve_tokens`, `keep_recent_tokens`.
- `[forges.<name>]` `kind` (`github` or `forgejo`), `url`, `token`, optional `default_repo`.
- `[mcp.<name>]` either `url` (streamable HTTP, optional `headers`) or `command` with `args` and `env` (stdio). Optional `tools` allowlist, `approve` list, and `timeout_secs`.

## Discord application

- Privileged intents: **Message Content** and **Server Members**.
- Invite with the `bot` and `applications.commands` scopes and these permissions: View Channels, Send Messages, Send Messages in Threads, Create Public Threads, Embed Links, Attach Files, Read Message History, Add Reactions, Use External Emojis, Manage Messages, Pin Messages, Manage Threads, Manage Channels, Manage Roles, Kick Members, Ban Members, Moderate Members (permission integer `2253226011651158`).
- Moderating a member requires the bot's highest role to be above the member's highest role. The bot reads a private thread only when it has been added to it and the person asking is a member.

## Running locally

```sh
cp .env.example .env   # fill it in
npm --prefix sandbox ci && npm --prefix sandbox run build
cargo run
```

`cargo test` runs the unit and mock-server tests. `cargo test -- --ignored` also runs live tests against the configured LLM and Exa.

## Deployment

Pushing to `main` runs `.forgejo/workflows/release.yml` on the NAS runner. It builds the binary and the sandbox with the toolchain, cargo, and npm caches, uploads them with `config.toml` as `reseam-bot-linux-x64.tar.gz` to the rolling `latest` release, and calls the Dokploy deploy webhook. Dokploy builds the `Dockerfile`, which installs git on `node:24-trixie-slim` and unpacks that release. Production environment variables live on the `bot` application in Dokploy, and the `bot-data` volume is mounted at `/var/lib/reseam-bot`.
