// Sign-in for the portal: OAuth 2.1 authorization code + PKCE as the first-party public client "recordem-web".
// Tokens live in localStorage; the access token is refreshed (rotating refresh token) shortly before it expires.
const CLIENT_ID = "recordem-web";
const SCOPE = "openid profile email offline_access meetings:read meetings:write transcripts:read audio:read calendars:read credentials:manage";
const TOKEN_URL = __TOKEN_URL__;
const KEY = "recordem.tokens";

type Tokens = { access: string; refresh: string | null; expiresAt: number };

const redirectUri = () => `${location.origin}/callback`;
const b64url = (b: ArrayBuffer | Uint8Array) => btoa(String.fromCharCode(...new Uint8Array(b as ArrayBuffer))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

function load(): Tokens | null {
  try { return JSON.parse(localStorage.getItem(KEY) ?? "null"); } catch { return null; }
}
function save(t: Tokens | null) {
  try { t ? localStorage.setItem(KEY, JSON.stringify(t)) : localStorage.removeItem(KEY); } catch { /* private mode: in-memory only */ }
  memory = t;
}
let memory: Tokens | null = load();

export const signedIn = () => !!memory;

export async function signIn(returnTo = location.pathname + location.search) {
  const verifier = b64url(crypto.getRandomValues(new Uint8Array(32)));
  const state = b64url(crypto.getRandomValues(new Uint8Array(16)));
  sessionStorage.setItem(`recordem.pkce.${state}`, JSON.stringify({ verifier, returnTo }));
  const challenge = b64url(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier)));
  const q = new URLSearchParams({
    response_type: "code", client_id: CLIENT_ID, redirect_uri: redirectUri(), scope: SCOPE, state,
    code_challenge: challenge, code_challenge_method: "S256", resource: __API_AUDIENCE__,
  });
  location.assign(`${__AUTH_URL__}/api/auth/oauth2/authorize?${q}`);
}

export function signOut() {
  save(null);
  location.assign("/");
}

async function tokenRequest(body: Record<string, string>): Promise<Tokens> {
  const res = await fetch(TOKEN_URL, {
    method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ client_id: CLIENT_ID, resource: __API_AUDIENCE__, ...body }),
  });
  const j = await res.json();
  if (!res.ok) throw new Error(j.error_description ?? j.error ?? `token: ${res.status}`);
  return { access: j.access_token, refresh: j.refresh_token ?? null, expiresAt: Date.now() + (j.expires_in ?? 3600) * 1000 };
}

/** Finishes the redirect back from the identity provider; returns where to go next. */
export async function completeSignIn(): Promise<string> {
  const q = new URLSearchParams(location.search);
  if (q.get("error")) throw new Error(q.get("error_description") ?? q.get("error")!);
  const state = q.get("state") ?? "";
  const saved = JSON.parse(sessionStorage.getItem(`recordem.pkce.${state}`) ?? "null") as { verifier: string; returnTo: string } | null;
  if (!saved) throw new Error("This sign-in link expired. Try again.");
  sessionStorage.removeItem(`recordem.pkce.${state}`);
  save(await tokenRequest({ grant_type: "authorization_code", code: q.get("code") ?? "", redirect_uri: redirectUri(), code_verifier: saved.verifier }));
  return saved.returnTo === "/callback" ? "/" : saved.returnTo;
}

let refreshing: Promise<Tokens> | null = null;

/** A valid access token, refreshed when it is about to expire; sends the user to sign in when that fails. */
export async function accessToken(): Promise<string> {
  const t = memory;
  if (!t) { await signIn(); throw new Error("signing in"); }
  if (t.expiresAt - Date.now() > 60_000) return t.access;
  if (!t.refresh) { await signIn(); throw new Error("signing in"); }
  refreshing ??= tokenRequest({ grant_type: "refresh_token", refresh_token: t.refresh }).finally(() => (refreshing = null));
  try {
    const n = await refreshing;
    save({ ...n, refresh: n.refresh ?? t.refresh });
    return n.access;
  } catch {
    save(null);
    await signIn();
    throw new Error("signing in");
  }
}
