// Settings: the workspace's machine credentials, for a backend that records on its users' behalf (the Desktop SDK).
// API keys are long-lived Bearer tokens; machine clients trade an id and secret for 1 h tokens (client_credentials).
// Either is shown once, when made. Only the workspace's owners and admins see this page work.
import type { components } from "@recordem/sdk";
import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import { signIn } from "./auth";
import { day } from "./format";
import { link } from "./router";

type ApiKey = components["schemas"]["ApiKey"];
type MachineClient = components["schemas"]["MachineClient"];
type Scope = ApiKey["scopes"][number];

const SCOPES: { scope: Scope; label: string; on: boolean }[] = [
  { scope: "meetings:write", label: "Create recordings (and upload tokens for desktop apps)", on: true },
  { scope: "meetings:read", label: "Read meetings and recordings", on: true },
  { scope: "transcripts:read", label: "Read transcripts and notes", on: true },
  { scope: "audio:read", label: "Download audio", on: false },
  { scope: "webhooks:manage", label: "Manage webhooks", on: true },
  { scope: "calendars:read", label: "Read calendars", on: false },
  { scope: "claims:write", label: "Claim meetings for an integration", on: false },
];

export function Settings() {
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const [clients, setClients] = useState<MachineClient[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [needsScope, setNeedsScope] = useState(false);

  const load = useCallback(async () => {
    const [k, c] = await Promise.all([api.GET("/v1/api-keys"), api.GET("/v1/machine-clients")]);
    const e = k.error ?? c.error;
    if (e) { setNeedsScope(e.error === "insufficient_scope"); setError(e.message); return; }
    setKeys(k.data!.items); setClients(c.data!.items); setError(null);
  }, []);
  useEffect(() => { void load(); }, [load]);

  return (
    <div className="shell">
      <header className="top">
        <a className="brand" {...link("/")}><span className="dot" /> Recordem</a>
        <a className="muted small" {...link("/")}>← All meetings</a>
      </header>
      <main className="settings">
        <h1>Settings</h1>
        {error && (needsScope
          ? <p className="muted">Managing credentials needs a fresh sign-in. <button className="ghost small" onClick={() => signIn("/settings")}>Sign in again</button></p>
          : <p className="error">{error}</p>)}
        {keys && clients && (
          <>
            <Section title="API keys" what="A key your backend sends as Authorization: Bearer rk_live_…. It acts in this workspace with the permissions you pick, until you revoke it."
              make={async (name, scopes) => { const r = await api.POST("/v1/api-keys", { body: { name, scopes } }); if (r.error) throw new Error(r.error.message); await load(); return [["API key", r.data.key]]; }}>
              {keys.map((k) => (
                <li key={k.id} className={k.revoked_at ? "revoked" : ""}>
                  <div><b>{k.name}</b> <code>{k.prefix}…</code></div>
                  <div className="muted small">{k.scopes.join(", ")} · made {day(new Date(k.created_at))} · {k.revoked_at ? `revoked ${day(new Date(k.revoked_at))}` : k.last_used_at ? `last used ${day(new Date(k.last_used_at))}` : "never used"}</div>
                  {!k.revoked_at && <button className="ghost small" onClick={async () => {
                    if (!confirm(`Revoke "${k.name}"? Whatever uses it stops working within a minute.`)) return;
                    await api.DELETE("/v1/api-keys/{id}", { params: { path: { id: k.id } } }); await load();
                  }}>Revoke</button>}
                </li>
              ))}
            </Section>
            <Section title="Machine clients" what="OAuth client_credentials: your backend trades the client id and secret for an access token that lasts an hour. For teams that prefer short-lived tokens."
              make={async (name, scopes) => { const r = await api.POST("/v1/machine-clients", { body: { name, scopes } }); if (r.error) throw new Error(r.error.message); await load(); return [["Client id", r.data.client_id], ["Client secret", r.data.client_secret], ["Token endpoint", r.data.token_endpoint]]; }}>
              {clients.map((c) => (
                <li key={c.client_id}>
                  <div><b>{c.name}</b> <code>{c.client_id}</code></div>
                  <div className="muted small">{c.scopes.join(", ")} · made {day(new Date(c.created_at))}</div>
                  <button className="ghost small" onClick={async () => {
                    if (!confirm(`Delete "${c.name}"? It gets no new tokens; the ones it has last up to an hour.`)) return;
                    await api.DELETE("/v1/machine-clients/{id}", { params: { path: { id: c.client_id } } }); await load();
                  }}>Delete</button>
                </li>
              ))}
            </Section>
          </>
        )}
        {!keys && !error && <p className="muted">Loading…</p>}
      </main>
    </div>
  );
}

/** A list of credentials and the form that makes one; what was made is shown once, to copy. */
function Section({ title, what, make, children }: {
  title: string; what: string; children: React.ReactNode[];
  make: (name: string, scopes: Scope[]) => Promise<[string, string][]>;
}) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<Scope[]>(SCOPES.filter((s) => s.on).map((s) => s.scope));
  const [made, setMade] = useState<[string, string][] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setError(null);
    try { setMade(await make(name.trim(), scopes)); setOpen(false); setName(""); } catch (e) { setError((e as Error).message); }
  };
  return (
    <section>
      <h2>{title}</h2>
      <p className="muted small">{what}</p>
      {made && (
        <div className="secret">
          <p><b>Copy it now:</b> it won't be shown again.</p>
          {made.map(([k, v]) => <div key={k}><span className="muted small">{k}</span><code>{v}</code><button className="ghost small" onClick={() => navigator.clipboard?.writeText(v)}>Copy</button></div>)}
          <button className="ghost small" onClick={() => setMade(null)}>Done</button>
        </div>
      )}
      {children.length > 0 ? <ul className="creds">{children}</ul> : <p className="muted small">None yet.</p>}
      {open ? (
        <form className="make" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
          <input placeholder="Name, e.g. Acme backend" value={name} onChange={(e) => setName(e.target.value)} maxLength={100} required autoFocus />
          {SCOPES.map((s) => (
            <label key={s.scope}><input type="checkbox" checked={scopes.includes(s.scope)}
              onChange={(e) => setScopes((x) => (e.target.checked ? [...x, s.scope] : x.filter((y) => y !== s.scope)))} /> {s.label}</label>
          ))}
          {error && <p className="error small">{error}</p>}
          <div><button disabled={!name.trim() || !scopes.length}>Create</button> <button type="button" className="ghost" onClick={() => setOpen(false)}>Cancel</button></div>
        </form>
      ) : <button className="ghost small" onClick={() => setOpen(true)}>New {title === "API keys" ? "API key" : "machine client"}</button>}
    </section>
  );
}
