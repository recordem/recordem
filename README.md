# Recordem

Open-source meeting memory. A desktop recorder notices when a call starts, records it on your machine (no bot joins
the call), transcribes it locally and sends the transcript and audio to your Recordem server. There you review every
meeting by date, with a generated title and summary, and any tool reads them through one API: the web app, the CLI,
an MCP client, or your own product through the SDK and webhooks.

*Record 'em* — and, in Portuguese, *recordem*: "remember".

**Status:** early development. See [AGENTS.md](AGENTS.md) for the layout and how to run it.

## Agents

Your meetings, for any agent, three ways over the same API:

- **MCP:** add `https://<your server's mcp>/mcp` as a remote MCP server (Claude, ChatGPT, editors). It asks you to sign
  in, then offers `search_transcripts`, `list_meetings`, `get_meeting`, `get_transcript` and `get_notes`.
- **CLI:** `recordem login --server <api url>`, then `recordem search "…"`, `recordem meeting <id>`,
  `recordem transcript <id>`, `recordem notes <id>` (`--json` on any of them). Install with `npm i -g recordem`
  (Node 22+), or run it once with `npx recordem`.
- **Skill:** `npx skills add recordem/recordem` teaches a coding agent (Claude Code, Codex, Cursor…) to use the CLI.

## License

The server (`apps/api`, `apps/auth`, `apps/worker`, `apps/web`, `apps/mcp`) is [AGPL-3.0](LICENSE). The recorder and
the packages you embed (`apps/recorder`, `packages/*`) are MIT. See [LICENSING.md](LICENSING.md).
