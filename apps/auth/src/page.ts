// The layout of the identity provider's pages.
export const page = (title: string, body: string, error?: string) => `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title} · Recordem</title>
<style>
  :root { color-scheme: light dark; --fg: #1d1d1f; --bg: #fafafa; --muted: #6e6e73; --accent: #d93025; --line: #d2d2d7; }
  @media (prefers-color-scheme: dark) { :root { --fg: #f5f5f7; --bg: #161617; --muted: #a1a1a6; --line: #3a3a3c; } }
  body { font: 15px/1.5 system-ui, sans-serif; color: var(--fg); background: var(--bg); display: grid; place-items: center; min-height: 100vh; margin: 0; padding: 0 16px; }
  main { width: 100%; max-width: 360px; }
  h1 { font-size: 20px; margin: 0 0 20px; } h1 span { color: var(--accent); }
  form { display: grid; gap: 12px; } label { display: grid; gap: 4px; color: var(--muted); font-size: 13px; }
  label.row { display: flex; align-items: center; gap: 8px; color: var(--fg); font-size: 15px; }
  input:not([type=checkbox]):not([type=radio]) { font: inherit; padding: 8px 10px; border: 1px solid var(--line); border-radius: 8px; background: transparent; color: var(--fg); }
  button { font: inherit; padding: 9px 14px; border: 0; border-radius: 8px; background: var(--fg); color: var(--bg); cursor: pointer; }
  button.ghost { background: transparent; color: var(--fg); border: 1px solid var(--line); }
  .error { color: var(--accent); margin: 0 0 12px; }
  p { margin: 0 0 12px; } .muted { color: var(--muted); } .small { font-size: 13px; } .lead { font-size: 16px; }
  .perms { list-style: none; margin: 0 0 20px; padding: 4px 14px; border: 1px solid var(--line); border-radius: 10px; }
  .perms li { padding: 9px 0 9px 26px; position: relative; } .perms li + li { border-top: 1px solid var(--line); }
  .perms li::before { content: "✓"; position: absolute; left: 2px; color: var(--muted); }
  form.buttons { grid-template-columns: 1fr 1fr; margin-bottom: 16px; }
</style></head>
<body><main><h1><span>●</span> ${title}</h1>${error ? `<p class="error">${error.replace(/</g, "&lt;")}</p>` : ""}${body}</main></body></html>`;
