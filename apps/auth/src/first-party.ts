// Clients that ship with Recordem get fixed, well-known ids ("recordem-web", later the CLI and the recorder), so they
// need no registration step. Upserted on boot straight into better-auth's oauthClient table (the same columns a
// registration fills): its admin endpoint needs a signed-in admin, and its validation refuses plain-http redirect
// URIs on LAN hosts, which a dev box uses. Public clients: PKCE, no secret, no consent screen.
import { SCOPES } from "@recordem/contract";
import type { Pool } from "pg";

type FirstParty = { id: string; name: string; redirectUris: string[]; type: "web" | "native" };

const scopes = ["openid", "profile", "email", "offline_access", ...Object.keys(SCOPES)];

export async function ensureFirstParty(pool: Pool, c: FirstParty) {
  await pool.query(
    `INSERT INTO "oauthClient" (id, "clientId", name, "redirectUris", scopes, "clientCredentialsScopes", "grantTypes", "responseTypes",
                                "tokenEndpointAuthMethod", "applicationType", "skipConsent", "requirePKCE", "softwareId", disabled,
                                "dpopBoundAccessTokens", "createdAt", "updatedAt")
     VALUES ($1, $1, $2, $3::jsonb, $4::jsonb, '[]', '["authorization_code","refresh_token"]', '["code"]',
             'none', $5, true, true, $1, false, false, now(), now())
     ON CONFLICT ("clientId") DO UPDATE SET name = excluded.name, "redirectUris" = excluded."redirectUris",
       scopes = excluded.scopes, "applicationType" = excluded."applicationType", "updatedAt" = now()`,
    [c.id, c.name, JSON.stringify(c.redirectUris), JSON.stringify(scopes), c.type],
  );
}
