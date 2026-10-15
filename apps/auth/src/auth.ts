// The identity provider: users, sessions, workspaces (better-auth organizations) and the OAuth 2.1 server that
// issues the JWT access tokens the API verifies against /api/auth/jwks.
import { oauthProvider } from "@better-auth/oauth-provider";
import { CLAIMS, SCOPES } from "@recordem/contract";
import { betterAuth } from "better-auth";
import { APIError } from "better-auth/api";
import { jwt, organization } from "better-auth/plugins";
import { Pool } from "pg";

const env = (k: string, d?: string) => process.env[k] ?? d ?? (() => { throw new Error(`${k} is not set`); })();
export const AUTH_URL = env("AUTH_URL", "http://localhost:39410");
export const API_URL = env("API_URL", "http://localhost:39411");

// The issuer the OAuth metadata announces is https for any non-loopback host (even on a plain-http dev box), so the
// JWTs are signed with that same value; it is an identifier, never fetched.
const ISSUER = (() => {
  const u = new URL(`${AUTH_URL}/api/auth`);
  if (!["localhost", "127.0.0.1", "[::1]"].includes(u.hostname)) u.protocol = "https:";
  return u.toString();
})();

export const pool = new Pool({ connectionString: env("AUTH_DATABASE_URL") });

/** The first workspace a user belongs to: the default when a session starts. */
async function firstWorkspace(userId: string): Promise<string | null> {
  const { rows } = await pool.query(`SELECT "organizationId" FROM member WHERE "userId" = $1 ORDER BY "createdAt" LIMIT 1`, [userId]);
  return rows[0]?.organizationId ?? null;
}

// Bound after `auth` exists: its hooks can't reference `auth` directly without making its type circular.
let createPersonalWorkspace: (userId: string) => Promise<string | null>;

export const auth = betterAuth({
  baseURL: AUTH_URL,
  secret: env("BETTER_AUTH_SECRET"),
  database: pool,
  emailAndPassword: { enabled: true },
  // The OAuth token endpoint replaces better-auth's own /token.
  disabledPaths: ["/token"],
  // The portal calls the token endpoint from its own origin.
  trustedOrigins: [AUTH_URL, ...(process.env.WEB_ORIGINS ?? "http://localhost:39412").split(",")],
  databaseHooks: {
    session: {
      create: {
        // A session starts in the user's first workspace; everyone gets a personal one on their first session.
        before: async (session) => {
          let ws = await firstWorkspace(session.userId);
          ws ??= await createPersonalWorkspace(session.userId);
          return { data: { ...session, activeOrganizationId: ws } };
        },
      },
    },
  },
  plugins: [
    organization(),
    jwt({ jwt: { issuer: ISSUER } }),
    oauthProvider({
      loginPage: "/sign-in",
      consentPage: "/consent",
      scopes: ["openid", "profile", "email", "offline_access", ...Object.keys(SCOPES)],
      resources: [API_URL],
      // The API is the one resource; scopes and the user's consent decide what a client gets, not a per-client link.
      enforcePerClientResources: false,
      // MCP clients (Claude…) and the CLI register themselves before anyone signs in.
      allowDynamicClientRegistration: true,
      allowUnauthenticatedClientRegistration: true,
      // A token acts in one workspace: the one active when the user authorized the client.
      postLogin: {
        page: "/select-workspace",
        shouldRedirect: ({ session }) => !session.activeOrganizationId,
        consentReferenceId: ({ session }) => {
          const ws = session.activeOrganizationId as string | undefined;
          if (!ws) throw new APIError("BAD_REQUEST", { error: "set_organization", error_description: "pick a workspace" });
          return ws;
        },
      },
      customAccessTokenClaims: ({ referenceId }) => (referenceId ? { [CLAIMS.workspace]: referenceId } : {}),
    }),
  ],
});

createPersonalWorkspace = async (userId) => {
  const org = await auth.api.createOrganization({ body: { name: "Personal", slug: `personal-${userId.slice(0, 8).toLowerCase()}`, userId } });
  return org?.id ?? null;
};
