// What a backend needs besides the calls: short-lived tokens from a client_credentials client, and checking that a
// webhook delivery came from Recordem. Web Crypto only, so it runs in Node 18+, Bun, Deno and edge runtimes.
import type { StreamEvent } from "./index";

/** A token function for createClient: trades a client_credentials client's id and secret for an access token at the
 * token endpoint, and keeps it until a minute before it expires. `resource` is the API's URL (the token's audience). */
export function clientCredentials(o: { tokenUrl: string; clientId: string; clientSecret: string; resource: string; scope?: string; fetch?: typeof fetch }) {
  let held: { token: string; until: number } | null = null;
  let pending: Promise<string> | null = null;
  return async () => {
    if (held && Date.now() < held.until) return held.token;
    pending ??= (async () => {
      const res = await (o.fetch ?? fetch)(o.tokenUrl, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({ grant_type: "client_credentials", client_id: o.clientId, client_secret: o.clientSecret, resource: o.resource, ...(o.scope ? { scope: o.scope } : {}) }),
      });
      const t = await res.json().catch(() => ({})) as { access_token?: string; expires_in?: number; error_description?: string; error?: string };
      if (!res.ok || !t.access_token) throw new Error(`client_credentials: ${t.error_description ?? t.error ?? res.status}`);
      held = { token: t.access_token, until: Date.now() + ((t.expires_in ?? 3600) - 60) * 1000 };
      return t.access_token;
    })().finally(() => { pending = null; });
    return pending;
  };
}

export class WebhookError extends Error {}

/** The event in a webhook delivery, once its `Recordem-Signature` (t=<unix s>,v1=<hex HMAC-SHA256 of "t.body">) checks
 * out with the endpoint's secret and is at most `toleranceS` old. Pass the raw body, exactly as received. */
export async function verifyWebhook(secret: string, body: string, signature: string | null | undefined, toleranceS = 300): Promise<StreamEvent> {
  const [, t, v1] = signature?.match(/^t=(\d+),v1=([0-9a-f]{64})$/) ?? [];
  if (!t || !v1) throw new WebhookError("no Recordem-Signature, or not t=…,v1=…");
  if (Math.abs(Date.now() / 1000 - Number(t)) > toleranceS) throw new WebhookError("signature too old");
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
  const mac = new Uint8Array(v1.match(/../g)!.map((h) => parseInt(h, 16)));
  if (!(await crypto.subtle.verify("HMAC", key, mac, new TextEncoder().encode(`${t}.${body}`)))) throw new WebhookError("signature doesn't match");
  return JSON.parse(body) as StreamEvent;
}
