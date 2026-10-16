// OAuth scopes the API understands. The identity provider advertises them; the API checks them per route.
export const SCOPES = {
  "meetings:read": "List and read meetings",
  "meetings:write": "Create and change meetings and recordings",
  "transcripts:read": "Read transcripts",
  "audio:read": "Download audio",
  "calendars:read": "Read connected calendars",
  "claims:write": "Claim meetings for an integration",
  "webhooks:manage": "Manage webhook endpoints",
} as const;
export type Scope = keyof typeof SCOPES;

/** Claim names Recordem adds to access tokens. */
export const CLAIMS = { workspace: "workspace", recording: "recording" } as const;

/** The scope of an upload token: never granted through OAuth, only minted with a recording (createRecording `upload`)
 * and good for that one recording, on the routes marked `x-upload`. */
export const UPLOAD_SCOPE = "recordings:upload";
