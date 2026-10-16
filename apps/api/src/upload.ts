// Upload tokens, for a desktop app sending one recording (the Desktop SDK). The api never signs tokens: it asks the
// identity provider, on its internal port, once it has checked the caller may write the recording.
const URL_ = process.env.AUTH_INTERNAL_URL ?? "http://localhost:39413";
const SECRET = process.env.INTERNAL_SECRET ?? "";

export async function uploadToken(claims: { sub: string; workspace: string; recording: string; client: string | null }) {
  const res = await fetch(`${URL_}/internal/upload-token`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${SECRET}` },
    body: JSON.stringify(claims),
  });
  if (!res.ok) throw new Error(`upload token: ${res.status} ${await res.text()}`);
  return (await res.json()) as { token: string; expires_at: string };
}
