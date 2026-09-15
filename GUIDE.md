# Reseam Bot guide

Reseam Bot is the AI assistant in the Reseam Discord server. It answers questions, reads and summarizes channels, and for the team it works with our repositories and Forgejo issues.

## Who can use it

| Who | What they get |
|---|---|
| Owners and the **Team** role | Everything in this guide |
| The **AI Access** role | Chat, reading and summarizing Discord, and Discord actions they are allowed to do themselves. No web, repositories, Forgejo, or code |
| Everyone else | The bot ignores them |

Moderation slash commands are separate and follow normal Discord permissions.

## Talking to it

- **Mention @Reseam** in any channel, or use **#bot**.
- **Reply to its answer** to continue the conversation. It keeps your earlier messages, its answers, and everything it looked up. A new mention starts a fresh conversation.
- **Say what you mean.** It does not read the channel automatically. It looks back on its own when a request depends on earlier messages, so name the channel or time range when it matters: "summarize #dev from the last 3 hours".
- **Attach files.** It reads images, PDFs, DOCX, and text files.
- **Add details mid-run** by replying again. It picks them up at its next step.
- If someone else replies to the same conversation while it is busy, their message gets ⏳ and runs afterwards.

## While it works

- The line under the answer shows what it is doing, the step number, and how long it has been running.
- **Stop** ends the run. Only the person who asked, owners, and people with Manage Messages can press it.
- A run can take up to 100 steps. On the last one it must answer with what it found so far.

## Approvals

The bot asks before doing anything with an effect outside the conversation:

- posting in a different channel
- any moderation action
- creating or changing things on Forgejo, GitHub, or other websites, including uploads

You get a card with **Approve**, **Approve for this run** (the same kind of action to the same place, for the rest of this request), and **Deny**. Only the person who asked can decide. Cards expire after 5 minutes and disappear once decided. After a Deny, the bot will not ask for that action again in the same run.

## What the team can ask for

- **Discord:** read channels and threads, find what someone said, open attachments, send messages, react, create threads, pin, and moderate. Every action is checked against your own Discord permissions.
- **Forgejo:** search and read issues and pull requests, create and edit issues, comment, label, and attach screenshots or videos. Changes show up as the `reseam-bot` account, which has write access to api, patches, reseam, website, and manager.
- **GitHub:** public data only.
- **Code:** clone a repository and read or search it.
- **Web:** search the web and read pages.
- **Data:** calculations and processing with Python, JSON, CSV, and SQLite in a private workspace for the conversation. Workspace files are deleted after 8 hours without use.

## Commands

- `/ask`: ask a question, with an optional file.
- `/summarize`: summarize a channel by message count or a time window like `3d`. The summary is posted in that channel.
- Right-click a message, then **Apps**:
  - **Summarize from here**: that message and what came after it.
  - **Ask about this**: opens a form for your question about the message.
  - **Create issue** (team): drafts a Forgejo issue from the message and asks before creating it.
- `/personality` (owners): extra tone and style instructions for the bot.
- `/mcp status` (team): shows the connected external services, such as web search.

## Moderation commands

| Command | Needs | Notes |
|---|---|---|
| `/warn` | Moderate Members | Sends the member a DM |
| `/timeout`, `/untimeout` | Moderate Members | Up to 28 days |
| `/kick` | Kick Members | Sends the member a DM |
| `/ban`, `/unban` | Ban Members | Optional duration up to 365 days. Works on people who already left |
| `/purge` | Manage Messages | Recent messages, optionally only from one user, containing some text, or from bots |
| `/slowmode`, `/lock`, `/unlock` | Manage Channels | Unlock restores the channel's permissions as they were |
| `/modlog` | Manage Server | Sets the channel where actions are logged |

## Good to know

- **Broad requests are slow.** Discord does not let bots search messages, so "everything X said about Y" means reading history page by page. A channel or time range makes it much faster.
- **Private threads** are only readable when both you and the bot are in them.
- **Memory has limits.** Conversations are forgotten after 30 days without activity, and very long ones get older parts summarized.
- **Only the person who asked gives orders.** Messages in the channel, files, and web pages are treated as information, never as instructions.
