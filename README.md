# Recordem

Open-source meeting memory. A desktop recorder notices when a call starts, records it on your machine (no bot joins
the call), transcribes it locally and sends the transcript and audio to your Recordem server. There you review every
meeting by date, with a generated title and summary, and any tool reads them through one API: the web app, the CLI,
an MCP client, or your own product through the SDK and webhooks.

*Record 'em* — and, in Portuguese, *recordem*: "remember".

**Status:** early development. See [AGENTS.md](AGENTS.md) for the layout and how to run it.

## License

The server (`apps/api`, `apps/auth`, `apps/worker`, `apps/web`) is [AGPL-3.0](LICENSE). The recorder and the packages
you embed (`apps/recorder`, `packages/*`) are MIT. See [LICENSING.md](LICENSING.md).
