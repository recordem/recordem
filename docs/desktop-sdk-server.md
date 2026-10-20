# The server side of the Desktop SDK

A company embeds Recordem the way Recall.ai's Desktop SDK works: its backend holds a machine credential and creates each
recording; its desktop app gets an upload token for that one recording and sends it; the backend hears back by webhook.
End to end: `bun run smoke:sdk` (scripts/sdk.ts).

## Machine credentials

Two kinds, managed by a workspace's owners and admins in the portal (Settings) or through the API. Either acts in one
workspace, with scopes picked from `MACHINE_SCOPES` (never `credentials:manage`), and shows as `machine: true` on
`GET /v1/me`.

| | API key | client_credentials client |
| --- | --- | --- |
| Looks like | `rk_live_…` (Bearer) | client id `rc_…` + secret `rcs_…`, traded for a 1 h JWT |
| Made with | `POST /v1/api-keys` | `POST /v1/machine-clients` |
| Kept in | `auth."apiKey"`: SHA-256 only | better-auth's `auth."oauthClient"`: secret hashed, workspace in `metadata.recordem_workspace` |
| The api verifies it | asks auth's internal port (`POST /internal/keys/verify`), keeps the answer 60 s | as any JWT (JWKS); claims `workspace` and `machine: true` |
| Revoked | `DELETE /v1/api-keys/{id}`: refused within 60 s everywhere (at once on the api process that revoked it) | `DELETE /v1/machine-clients/{id}`: no new tokens; issued ones live up to 1 h |
| SDK | `createClient({ baseUrl, token: "rk_live_…" })` | `createClient({ baseUrl, token: clientCredentials({ tokenUrl, clientId, clientSecret, resource }) })` |

Identity stays separate: auth owns both stores and the admin check (`member.role` owner/admin); the api only verifies
and passes the person's id and workspace to auth's internal port (`/internal/admin/*`, shared secret). A machine
credential or an upload token never manages credentials. Dynamic client registration can't set client metadata nor
client_credentials scopes, so only `machine.ts` makes machine clients.

A client_credentials token request must name the API as `resource` (else better-auth issues an opaque token the api
can't read): `grant_type=client_credentials&client_id=…&client_secret=…&resource=<API_URL>` (client_secret_post).

## A recording made by a backend

`POST /v1/recordings` with `upload: true` returns `upload.token` (6 h; ask again with the same `id` for a fresh one),
plus `ref`, `metadata` (≤ 20 keys) and `user` (who records, in the backend's terms: an email, its own user id). The
recording's owner is the machine (`key:<id>` or the client id): only it writes to the recording besides the token.

Events, in order: `recording.created` → `recording.started` (first live batch, or the final transcript when there was
none) → `transcript.live`, `participants.updated`, `transcript.speakers` → `recording.ended` (a live batch with
`ended: true`) → `transcript.final` → `track.ready` → `recording.uploaded` (once the final transcript and the `mix`
track are both in, whichever came last) → `notes.ready`. Or `recording.failed`, reported by the desktop app; ignored
after `recording.uploaded`. `recording.started`, `recording.uploaded` and `recording.failed` carry
`{ ref, metadata, user }` (and `reason`, `message` for a failure).

## What the desktop app sends, with only the upload token

Every route `apps/recorder/src-tauri/src/api.rs` calls today, and what an upload token may do:

| Route | The recorder uses it for | Upload token |
| --- | --- | --- |
| `GET /v1/me` | checking its token works | yes: answers `scopes: ["recordings:upload"]` |
| `POST /v1/meetings/context` | finding the calendar meeting a call belongs to | no: the backend chose the meeting; skip it |
| `POST /v1/recordings` | creating the recording | no: the backend created it; the app gets its `id` and token |
| `POST /v1/recordings/{id}/live` | live transcript, participants, speaker names, reader reports, `ended`; the answer has `n` (resend from) and `stop` | yes |
| `PUT /v1/recordings/{id}/transcript` | the final transcript (ends the recording) | yes |
| `PUT /v1/recordings/{id}/tracks/{mix,mic_hq}` | audio (AAC .m4a) | yes |
| `PUT /v1/recordings/{id}/reader-dumps/{t_ms}-{good,bad}.json.gz` | broken call-app snapshots, when allowed | yes |
| `GET /v1/recorder/profiles` | reader profiles | public, no token |
| `GET /v1/recordings/{id}` | new: its state (`uploaded_at`, `failed`, `ended_at`) | yes |
| `POST /v1/recordings/{id}/stop` | new: ask itself to stop (the next live answer says `stop: true`) | yes |
| `POST /v1/recordings/{id}/failure` | new: `{ reason: permission_denied \| capture_failed \| transcription_failed \| upload_failed \| other, message }` | yes |

An upload token reads nothing else: no transcript, no meetings, no other recording (403).
