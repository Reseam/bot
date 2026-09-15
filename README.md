# Reseam Bot

Discord bot for the Reseam team with a built-in AI agent, plus moderation commands.

## Using it

Only owners (`access.owner_ids`) and members with a team role (`access.team_role_ids`) can use the AI. Moderation commands use normal Discord permissions instead.

- **Mention the bot** or **reply to one of its messages**. It reads recent channel history, the message you replied to, and any attached images, PDFs, DOCX, or text files, then streams its answer. Press **Stop** to end a run.
- **Reply to its answer** to continue the same conversation. Earlier turns and tool results are kept in SQLite; long conversations are summarized automatically.
- **Reply to its message while it is still working** to steer the run. The bot reacts with 👀 when your message is picked up.
- **Approvals.** Actions with side effects (shell commands, creating issues or comments, moderation, posting in another channel, MCP tools listed under `approve`) post an Approve / Approve for this run / Deny prompt. Only the person who started the run can answer it.

### Commands

| Command | Who | What |
|---|---|---|
| `/ask prompt [file]` | team | Ask the agent something |
| `/summarize [channel] [messages] [since]` | team | Summarize up to 500 messages, optionally limited by a duration like `2h` |
| `Summarize from here` (message menu) | team | Summarize from a message to now |
| `Ask about this` (message menu) | team | Ask a question about a message |
| `Create issue` (message menu) | team | Draft an issue from a message and create it after approval |
| `/mcp status` | team | MCP server status and tools |
| `/mcp reconnect server` | owners | Reconnect an MCP server |
| `/warn`, `/note`, `/timeout`, `/untimeout`, `/cases`, `/case` | Moderate Members | |
| `/kick` | Kick Members | |
| `/ban`, `/unban` | Ban Members | `/ban` takes an optional duration for temporary bans |
| `/purge` | Manage Messages | Bulk delete up to 100 recent messages |
| `/slowmode`, `/lock`, `/unlock` | Manage Channels | |
| `/case-delete`, `/modlog` | Manage Server | `/modlog channel` sets where cases are posted |

### Agent tools

- Discord: read messages, get a message, view attachments, list channels, server and member info, member search, send messages, react, create threads, pin.
- GitHub and Forgejo (`forges` in config): search issues and pull requests, read an issue with comments, read a pull request with its diff, create issues, comment.
- Repositories: clone any HTTPS repository into the data directory, then list, read, and grep it.
- Shell: `bash` in `DATA_DIR/workspace` or a cloned repository, with a cleared environment and a timeout. Every command needs approval.
- Moderation: warn, timeout, kick, ban, delete a message, purge, look up cases. The person who started the run needs the matching Discord permission.
- MCP: every allowed tool from servers configured under `[mcp.*]`.

## Configuration

`config.toml` is committed and only references environment variables with `${NAME}` or `${NAME:-default}`. A missing variable without a default stops startup with an error naming it. Copy `.env.example` to `.env` for local runs.

| Variable | Purpose |
|---|---|
| `DISCORD_TOKEN` | Bot token |
| `DISCORD_GUILD_ID` | Server the slash commands register to |
| `DISCORD_OWNER_ID`, `DISCORD_TEAM_ROLE_ID` | Who can use the AI |
| `DATA_DIR` | SQLite database, cloned repositories, shell workspace |
| `LLM_BASE_URL`, `LLM_API_KEY`, `LLM_MODEL` | Any OpenAI-compatible Chat Completions endpoint |
| `FORGEJO_URL`, `FORGEJO_TOKEN`, `GITHUB_TOKEN` | Forge access; without a token, only public reads work |
| `EXA_API_KEY` | Exa MCP server |

Notable settings in `config.toml`:

- `[llm]` `context_window` and `max_output_tokens` drive compaction. Set `vision = false` for models without image input. `extra_body` is merged into every request, e.g. `extra_body = { reasoning = { effort = "medium" } }`.
- `[agent]` `max_turns`, `history_messages`, `conversation_retention_days` (default 30), `compaction_reserve_tokens`, `keep_recent_tokens`.
- `[forges.<name>]` `kind` (`github` or `forgejo`), `url`, `token`, optional `default_repo`.
- `[shell]` `enabled`, `timeout_secs`.
- `[mcp.<name>]` either `url` (streamable HTTP, optional `headers`) or `command` with `args` and `env` (stdio). Optional `tools` allowlist, `approve` list, and `timeout_secs`.

## Discord application

- Privileged intents: **Message Content** and **Server Members**.
- Invite with the `bot` and `applications.commands` scopes and these permissions: View Channels, Send Messages, Send Messages in Threads, Create Public Threads, Embed Links, Attach Files, Read Message History, Add Reactions, Use External Emojis, Manage Messages, Pin Messages, Manage Threads, Manage Channels, Manage Roles, Kick Members, Ban Members, Moderate Members (permission integer `2253226011651158`).
- Moderating a member requires the bot's highest role to be above the member's highest role.

## Running locally

```sh
cp .env.example .env   # fill it in
cargo run
```

`cargo test` runs the unit and mock-server tests. `cargo test -- --ignored` also runs live tests against the configured LLM, Exa, and GitHub.

## Deployment

Pushing to `main` runs `.forgejo/workflows/image.yml` on the NAS runner: kaniko builds the Dockerfile, pushes `git.reseam.app/reseam/bot:latest`, and calls the Dokploy deploy webhook. Dokploy only pulls and runs the image. Production environment variables live on the `bot` application in Dokploy, and the `bot-data` volume is mounted at `/var/lib/reseam-bot`.
