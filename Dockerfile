# One image for the server: auth (identity provider), api, and the portal's static files (served by apps/web/serve.ts).
# Each Deployment runs its own command; migrations run as initContainers. The portal is built in a first stage with
# the public addresses baked in (Vite); the final stage installs on its own, since bun's isolated linker leaves
# symlinks a multi-stage COPY would break.
FROM oven/bun:1.3.11 AS web
WORKDIR /app
ARG AUTH_URL=https://auth.recordem.dev
ARG API_URL=https://api.recordem.dev
COPY package.json bun.lock tsconfig.base.json ./
COPY packages/contract/package.json packages/contract/
COPY packages/db/package.json packages/db/
COPY packages/sdk/package.json packages/sdk/
COPY apps/auth/package.json apps/auth/
COPY apps/api/package.json apps/api/
COPY apps/worker/package.json apps/worker/
COPY apps/web/package.json apps/web/
RUN bun install --frozen-lockfile
COPY packages packages
COPY apps/web apps/web
RUN cd apps/web && AUTH_URL=$AUTH_URL API_URL=$API_URL VITE_API_BASE=$API_URL VITE_TOKEN_URL=$AUTH_URL/api/auth/oauth2/token bunx vite build

FROM oven/bun:1.3.11
WORKDIR /app
COPY package.json bun.lock tsconfig.base.json ./
COPY packages/contract/package.json packages/contract/
COPY packages/db/package.json packages/db/
COPY packages/sdk/package.json packages/sdk/
COPY apps/auth/package.json apps/auth/
COPY apps/api/package.json apps/api/
COPY apps/worker/package.json apps/worker/
COPY apps/web/package.json apps/web/
RUN bun install --frozen-lockfile
COPY packages packages
COPY apps/auth apps/auth
COPY apps/api apps/api
COPY apps/worker apps/worker
# The reader profiles the api serves to recorders (GET /v1/recorder/profiles).
COPY apps/recorder/crates/scrape/profiles apps/recorder/crates/scrape/profiles
COPY apps/web/serve.ts apps/web/
COPY --from=web /app/apps/web/dist apps/web/dist
ENV NODE_ENV=production
USER bun
CMD ["bun", "apps/api/src/index.ts"]
