# Recordem

Open-source meeting memory: a local desktop recorder, a server that owns meetings, transcripts and calendars, and one
OpenAPI contract that every client (web, CLI, MCP, SDK, integrations) is a thin layer over.

## Layout

```
apps/auth/          identity provider: better-auth, organizations (= workspaces), OAuth 2.1 provider, JWKS
apps/api/           Hono + @hono/zod-openapi: REST (/v1) and SSE (/v1/stream); verifies tokens, never logs anyone in
apps/worker/        BullMQ workers: webhook delivery; meeting notes from a language model (any OpenAI-compatible API)
apps/web/           portal (Vite + React): calendar, meeting page, live transcript; a client of packages/sdk
apps/recorder/      macOS menu-bar recorder (Tauri 2, Rust): detects calls, records, transcribes locally (see its README)
packages/contract/  zod schemas + route definitions → openapi.json
packages/sdk/       TypeScript client: types generated from openapi.json (`bun run --filter @recordem/sdk generate`)
packages/db/        Drizzle schema + migrations of the api database
compose.yaml        Postgres and Redis for dev and self-hosting
```

## Rules

- **Contract first.** A route is defined in `packages/contract` before `apps/api` implements it; clients only call
  what is in `openapi.json`. `/v1` only grows; breaking changes go to `/v2`.
- **Identity is separate.** `apps/auth` owns users, sessions, workspaces and OAuth clients in schema `auth`, under its own role.
  `apps/api` only verifies JWTs against the auth JWKS and reads `sub`, `workspace` and `scope`.
- **Every api row has a `workspace_id`**, and every query filters by it.
- **Integrations are clients**: they keep their own data and store Recordem ids plus the opaque `ref` they gave us.
- **Self-hosting is first-class.** Hosted-only concerns (billing, limits) plug in from outside this repository.

## Dev

```bash
cp .env.example .env && docker compose up -d && bun install
bun run db:migrate && bun run dev        # auth :39410, api :39411
bun run smoke                            # OAuth sign-up → token → record a call → read it back → isolation
```

The portal runs on https (a self-signed cert in `apps/web/.cert/`): OAuth refuses plain-http redirect URIs off
loopback. Its dev server proxies `/v1`, `/files` and the token endpoint (`/idp`) so the browser sees one origin.

Ports are uncommon on purpose (auth 39410, api 39411, web 39412; Postgres 39420, Redis 39421).

## License

Server apps AGPL-3.0; `apps/recorder` and `packages/*` MIT. See [LICENSING.md](LICENSING.md).
