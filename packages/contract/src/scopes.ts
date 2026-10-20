// OAuth scopes the API understands. The identity provider advertises them; the API checks them per route.
export const SCOPES = {
  "meetings:read": "List and read meetings",
  "meetings:write": "Create and change meetings and recordings",
  "transcripts:read": "Read transcripts",
  "audio:read": "Download audio",
  "calendars:read": "Read connected calendars",
  "claims:write": "Claim meetings for an integration",
  "webhooks:manage": "Manage webhook endpoints",
  "credentials:manage": "Create and revoke the workspace's API keys and machine clients (admins only)",
} as const;
export type Scope = keyof typeof SCOPES;

/** Claim names Recordem adds to access tokens. */
export const CLAIMS = { workspace: "workspace", recording: "recording", machine: "machine" } as const;

/** Scopes a machine credential (an API key, a client_credentials client) may hold: what a backend does, never managing
 * credentials itself. */
export const MACHINE_SCOPES = ["meetings:read", "meetings:write", "transcripts:read", "audio:read", "calendars:read", "claims:write", "webhooks:manage"] as const satisfies readonly Scope[];

/** How an API key starts: what tells the api it is not a JWT. */
export const API_KEY_PREFIX = "rk_live_";

/** The scope of an upload token: never granted through OAuth, only minted with a recording (createRecording `upload`)
 * and good for that one recording, on the routes marked `x-upload`. */
export const UPLOAD_SCOPE = "recordings:upload";
