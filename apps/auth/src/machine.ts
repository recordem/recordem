// Machine credentials: how a backend acts in a workspace with no person signed in. Two kinds, one outcome (an access
// the api verifies, with `workspace`, scopes and the `machine` claim, and no person as its subject):
// - API keys (`rk_live_…`): long-lived Bearer tokens. Only their SHA-256 is kept; the api asks here (internal port)
//   what a key is, and keeps the answer for up to a minute.
// - client_credentials clients: confidential OAuth clients of better-auth's provider, written straight into its
//   oauthClient table (as first-party.ts does), with the workspace in their metadata. They trade id and secret for a
//   JWT at the token endpoint; dynamic registration can't set that metadata nor any client_credentials scope.
// Both are managed by a workspace's owners and admins through the api (/v1/api-keys, /v1/machine-clients).
import { API_KEY_PREFIX, MACHINE_SCOPES } from "@recordem/contract";
import { createHash, randomBytes } from "node:crypto";
import { pool, WORKSPACE_KEY } from "./auth";

const random = (bytes: number) => randomBytes(bytes).toString("base64url");
const sha256hex = (s: string) => createHash("sha256").update(s).digest("hex");
/** How better-auth's provider stores a client secret ("hashed"): SHA-256, base64url without padding. */
const clientSecretHash = (s: string) => createHash("sha256").update(s).digest("base64url");

export async function ensureMachineTables() {
  await pool.query(`CREATE TABLE IF NOT EXISTS "apiKey" (
    id text PRIMARY KEY,
    "workspaceId" text NOT NULL REFERENCES organization(id) ON DELETE CASCADE,
    name text NOT NULL,
    prefix text NOT NULL,
    hash text NOT NULL UNIQUE,
    scopes jsonb NOT NULL,
    "createdBy" text NOT NULL,
    "createdAt" timestamptz NOT NULL DEFAULT now(),
    "lastUsedAt" timestamptz,
    "revokedAt" timestamptz
  )`);
  await pool.query(`CREATE INDEX IF NOT EXISTS "apiKey_workspace" ON "apiKey" ("workspaceId", "createdAt" DESC)`);
}

/** Only scopes a machine may hold, each once, in a stable order. */
export function machineScopes(v: unknown): string[] | null {
  if (!Array.isArray(v) || !v.length) return null;
  if (!v.every((s) => (MACHINE_SCOPES as readonly string[]).includes(s))) return null;
  return MACHINE_SCOPES.filter((s) => v.includes(s));
}

/** Is this person an owner or admin of the workspace? (better-auth's member.role may list several, comma-separated.) */
export async function isAdmin(userId: string, workspace: string) {
  const { rows } = await pool.query(`SELECT role FROM member WHERE "userId" = $1 AND "organizationId" = $2`, [userId, workspace]);
  return rows.some((r: { role: string }) => r.role.split(",").some((x) => ["owner", "admin"].includes(x.trim())));
}

type KeyRow = { id: string; name: string; prefix: string; scopes: string[]; createdBy: string; createdAt: Date; lastUsedAt: Date | null; revokedAt: Date | null };
const keyView = (k: KeyRow) => ({
  id: k.id, name: k.name, prefix: k.prefix, scopes: k.scopes, created_by: k.createdBy, created_at: k.createdAt.toISOString(),
  last_used_at: k.lastUsedAt?.toISOString() ?? null, revoked_at: k.revokedAt?.toISOString() ?? null,
});

export async function listKeys(workspace: string) {
  const { rows } = await pool.query(`SELECT * FROM "apiKey" WHERE "workspaceId" = $1 ORDER BY "createdAt" DESC`, [workspace]);
  return rows.map(keyView);
}

export async function createKey(workspace: string, name: string, scopes: string[], createdBy: string) {
  const key = `${API_KEY_PREFIX}${random(32)}`;
  const { rows } = await pool.query(
    `INSERT INTO "apiKey" (id, "workspaceId", name, prefix, hash, scopes, "createdBy") VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7) RETURNING *`,
    [`key_${random(9)}`, workspace, name, key.slice(0, API_KEY_PREFIX.length + 4), sha256hex(key), JSON.stringify(scopes), createdBy],
  );
  return { ...keyView(rows[0]), key };
}

export async function revokeKey(workspace: string, id: string) {
  const { rowCount } = await pool.query(`UPDATE "apiKey" SET "revokedAt" = coalesce("revokedAt", now()) WHERE id = $1 AND "workspaceId" = $2`, [id, workspace]);
  return !!rowCount;
}

/** What a key is, when it is a live one; its last use is written at most once a minute. */
export async function verifyKey(key: string) {
  if (!key.startsWith(API_KEY_PREFIX)) return null;
  const { rows } = await pool.query(`SELECT * FROM "apiKey" WHERE hash = $1 AND "revokedAt" IS NULL`, [sha256hex(key)]);
  const k = rows[0] as (KeyRow & { workspaceId: string }) | undefined;
  if (!k) return null;
  if (!k.lastUsedAt || Date.now() - k.lastUsedAt.getTime() > 60_000) await pool.query(`UPDATE "apiKey" SET "lastUsedAt" = now() WHERE id = $1`, [k.id]);
  return { id: k.id, workspace: k.workspaceId, scopes: k.scopes };
}

type ClientRow = { clientId: string; name: string | null; clientCredentialsScopes: string[] | null; userId: string | null; createdAt: Date; metadata: Record<string, string> | null };
const clientView = (c: ClientRow) => ({
  client_id: c.clientId, name: c.name ?? c.clientId, scopes: c.clientCredentialsScopes ?? [],
  created_by: c.metadata?.recordem_created_by ?? null, created_at: c.createdAt.toISOString(),
});

export async function listClients(workspace: string) {
  const { rows } = await pool.query(
    `SELECT * FROM "oauthClient" WHERE "referenceId" = $1 AND metadata->>'${WORKSPACE_KEY}' = $1 ORDER BY "createdAt" DESC`, [workspace]);
  return rows.map(clientView);
}

export async function createClient(workspace: string, name: string, scopes: string[], createdBy: string) {
  const clientId = `rc_${random(18)}`;
  const secret = `rcs_${random(32)}`;
  const { rows } = await pool.query(
    `INSERT INTO "oauthClient" (id, "clientId", "clientSecret", name, "redirectUris", scopes, "clientCredentialsScopes", "grantTypes", "responseTypes",
                                "tokenEndpointAuthMethod", "applicationType", "skipConsent", "requirePKCE", disabled, "dpopBoundAccessTokens",
                                "referenceId", metadata, "createdAt", "updatedAt")
     VALUES ($1, $1, $2, $3, '[]', $4::jsonb, $4::jsonb, '["client_credentials"]', '[]', 'client_secret_post', 'web', true, false, false, false,
             $5, $6::jsonb, now(), now())
     RETURNING *`,
    [clientId, clientSecretHash(secret), name, JSON.stringify(scopes), workspace, JSON.stringify({ [WORKSPACE_KEY]: workspace, recordem_created_by: createdBy })],
  );
  return { ...clientView(rows[0]), client_secret: secret };
}

export async function deleteClient(workspace: string, clientId: string) {
  const { rowCount } = await pool.query(
    `DELETE FROM "oauthClient" WHERE "clientId" = $1 AND "referenceId" = $2 AND metadata->>'${WORKSPACE_KEY}' = $2`, [clientId, workspace]);
  return !!rowCount;
}
