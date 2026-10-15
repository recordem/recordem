# Recordem

Open-source meeting memory: a local desktop recorder, a server that owns meetings, transcripts and calendars, and one
OpenAPI contract that every client (web, CLI, MCP, SDK, integrations) is a thin layer over.

## Layout

```
apps/auth/          identity provider: better-auth, organizations (= workspaces), OAuth 2.1 provider, JWKS
apps/api/           Hono + @hono/zod-openapi: REST (/v1), later MCP (/mcp) and SSE; verifies tokens, never logs anyone in
apps/worker/        BullMQ workers (planned)
apps/web/           portal, a client of packages/sdk (planned)
apps/recorder/      desktop recorder, Tauri 2 (planned)
packages/contract/  zod schemas + route definitions → openapi.json
packages/db/        Drizzle schema + migrations of the api database
compose.yaml        Postgres and Redis for dev and self-hosting
```

## Rules

- **Contract first.** A route is defined in `packages/contract` before `apps/api` implements it; clients only call
  what is in `openapi.json`. `/v1` only grows; breaking changes go to `/v2`.
- **Identity is separate.** `apps/auth` owns users, sessions, workspaces and OAuth clients in its own database.
  `apps/api` only verifies JWTs against the auth JWKS and reads `sub`, `workspace` and `scope`.
- **Every api row has a `workspace_id`**, and every query filters by it.
- **Integrations are clients**: they keep their own data and store Recordem ids plus the opaque `ref` they gave us.
- **Self-hosting is first-class.** Hosted-only concerns (billing, limits) plug in from outside this repository.

## Dev

```bash
cp .env.example .env && docker compose up -d && bun install
bun run db:migrate && bun run dev        # auth :39410, api :39411
bun run smoke                            # sign up → OAuth (PKCE) → token → GET /v1/me
```

Ports are uncommon on purpose (auth 39410, api 39411, web 39412; Postgres 39420, Redis 39421).

## License

Server apps AGPL-3.0; `apps/recorder` and `packages/*` MIT. See [LICENSING.md](LICENSING.md).
