// Settings: keys for the workspace's own apps, e.g. a backend that records calls on its users' behalf (the Desktop SDK).
// Keys (API keys) are long-lived Bearer tokens; the advanced kind (machine clients) trades an id and secret for 1 h
// tokens (client_credentials). Either is shown once, when made. Only the workspace's owners and admins see this page work.
import type { components } from "@recordem/sdk";
import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import { signIn } from "./auth";
import { link } from "./router";

type ApiKey = components["schemas"]["ApiKey"];
type MachineClient = components["schemas"]["MachineClient"];
type Scope = ApiKey["scopes"][number];

const SCOPES: { scope: Scope; label: string; hint: string; on: boolean }[] = [
  { scope: "meetings:write", label: "Record calls", hint: "Start recordings, also from your desktop app", on: true },
  { scope: "meetings:read", label: "See meetings", hint: "Titles, times, people and recordings", on: true },
  { scope: "transcripts:read", label: "Read transcripts and notes", hint: "What was said, and the summary", on: true },
  { scope: "webhooks:manage", label: "Get notified", hint: "Your server hears when a meeting is ready", on: true },
  { scope: "audio:read", label: "Download audio", hint: "The recording's sound files", on: false },
  { scope: "calendars:read", label: "See calendars", hint: "Upcoming events from connected calendars", on: false },
  { scope: "claims:write", label: "Link meetings to its records", hint: "E.g. attach a meeting to a candidate", on: false },
];
const label = (s: Scope) => SCOPES.find((x) => x.scope === s)?.label ?? s;

const date = (iso: string) => {
  const d = new Date(iso);
  return d.toLocaleDateString([], { day: "numeric", month: "short", ...(d.getFullYear() !== new Date().getFullYear() && { year: "numeric" }) });
};

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
        <p className="muted lead">Let your own apps work with this workspace: record calls, read meetings, hear when one is ready.</p>
        {error && (needsScope
          ? <p className="muted">To manage keys, sign in again. <button className="ghost small" onClick={() => signIn("/settings")}>Sign in</button></p>
          : <p className="error">{error}</p>)}
        {keys && clients && (
          <>
            <Section title="Keys" noun="key" empty="No keys yet. Create one for each app, so you can turn one off without touching the others."
              what="An app uses its key to act in this workspace, with only the permissions you give it. It works until you revoke it."
              make={async (name, scopes) => { const r = await api.POST("/v1/api-keys", { body: { name, scopes } }); if (r.error) throw new Error(r.error.message); await load(); return [["Key", r.data.key]]; }}>
              {keys.map((k) => (
                <Item key={k.id} name={k.name} id={`${k.prefix}…`} scopes={k.scopes} off={!!k.revoked_at}
                  when={k.revoked_at ? `Revoked ${date(k.revoked_at)}` : `Created ${date(k.created_at)} · ${k.last_used_at ? `last used ${date(k.last_used_at)}` : "never used"}`}
                  action={k.revoked_at ? undefined : ["Revoke", async () => {
                    if (!confirm(`Revoke "${k.name}"? The app using it stops working within a minute.`)) return;
                    await api.DELETE("/v1/api-keys/{id}", { params: { path: { id: k.id } } }); await load();
                  }]} />
              ))}
            </Section>
            <details className="advanced" open={clients.length > 0}>
              <summary>Advanced: expiring credentials</summary>
              <Section title="Expiring credentials" noun="credential" empty="None yet."
                what="For security policies that don't allow keys that last forever: the app trades an ID and a secret for an access token that expires after an hour (OAuth client credentials)."
                make={async (name, scopes) => { const r = await api.POST("/v1/machine-clients", { body: { name, scopes } }); if (r.error) throw new Error(r.error.message); await load(); return [["ID", r.data.client_id], ["Secret", r.data.client_secret], ["Token URL", r.data.token_endpoint]]; }}>
                {clients.map((c) => (
                  <Item key={c.client_id} name={c.name} id={c.client_id} scopes={c.scopes} when={`Created ${date(c.created_at)}`}
                    action={["Delete", async () => {
                      if (!confirm(`Delete "${c.name}"? It gets no new tokens; the ones it has work for up to an hour.`)) return;
                      await api.DELETE("/v1/machine-clients/{id}", { params: { path: { id: c.client_id } } }); await load();
                    }]} />
                ))}
              </Section>
            </details>
          </>
        )}
        {!keys && !error && <p className="muted">Loading…</p>}
      </main>
    </div>
  );
}

function Item({ name, id, scopes, when, off, action }: {
  name: string; id: string; scopes: Scope[]; when: string; off?: boolean; action?: [string, () => Promise<void>];
}) {
  return (
    <li className={off ? "off" : ""}>
      <div className="cred-head"><b>{name}</b> <code>{id}</code></div>
      <div className="perms">{scopes.map((s) => <span key={s}>{label(s)}</span>)}</div>
      <div className="muted small">{when}</div>
      {action && <button className="ghost small" onClick={action[1]}>{action[0]}</button>}
    </li>
  );
}

/** A list of credentials and the form that makes one; what was made is shown once, to copy. */
function Section({ title, noun, what, empty, make, children }: {
  title: string; noun: string; what: string; empty: string; children: React.ReactNode[];
  make: (name: string, scopes: Scope[]) => Promise<[string, string][]>;
}) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<Scope[]>(SCOPES.filter((s) => s.on).map((s) => s.scope));
  const [made, setMade] = useState<[string, string][] | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const submit = async () => {
    setError(null);
    try { setMade(await make(name.trim(), scopes)); setOpen(false); setName(""); } catch (e) { setError((e as Error).message); }
  };
  const copy = (k: string, v: string) => { void navigator.clipboard?.writeText(v); setCopied(k); };
  return (
    <section>
      <div className="section-head">
        <div><h2>{title}</h2><p className="muted small">{what}</p></div>
        {!open && <button className="small" onClick={() => setOpen(true)}>New {noun}</button>}
      </div>
      {made && (
        <div className="secret">
          <p><b>Copy it now.</b> For your safety, it won't be shown again.</p>
          {made.map(([k, v]) => <div key={k}><span className="muted small">{k}</span><code>{v}</code>
            <button className="ghost small" onClick={() => copy(k, v)}>{copied === k ? "Copied" : "Copy"}</button></div>)}
          <div><button className="ghost small" onClick={() => { setMade(null); setCopied(null); }}>Done</button></div>
        </div>
      )}
      {open && (
        <form className="make" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
          <label className="field">Name
            <input placeholder="The app that will use it, e.g. Hiring app" value={name} onChange={(e) => setName(e.target.value)} maxLength={100} required autoFocus />
          </label>
          <fieldset>
            <legend>What it can do</legend>
            {SCOPES.map((s) => (
              <label key={s.scope} className="perm"><input type="checkbox" checked={scopes.includes(s.scope)}
                onChange={(e) => setScopes((x) => (e.target.checked ? [...x, s.scope] : x.filter((y) => y !== s.scope)))} />
                <span>{s.label}<span className="muted small">{s.hint}</span></span></label>
            ))}
          </fieldset>
          {error && <p className="error small">{error}</p>}
          <div><button disabled={!name.trim() || !scopes.length}>Create {noun}</button> <button type="button" className="ghost" onClick={() => setOpen(false)}>Cancel</button></div>
        </form>
      )}
      {children.length > 0 ? <ul className="creds">{children}</ul> : !open && <p className="empty muted small">{empty}</p>}
    </section>
  );
}
