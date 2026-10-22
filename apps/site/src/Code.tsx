// Code samples with just enough color to read: comments, strings and a handful of keywords.
import { Fragment, useState } from "react";

const TOKEN = /(\/\/.*$|^\s*#.*$|"[^"\n]*"|`[^`\n]*`|\b(?:import|from|const|await|async|if|export|default)\b)/gm;

function highlight(src: string) {
  return src.split(TOKEN).map((part, i) => {
    if (!part) return null;
    if (i % 2 === 0) return <Fragment key={i}>{part}</Fragment>;
    const kind = part.trimStart().startsWith("//") || part.trimStart().startsWith("#") ? "c" : /^["`]/.test(part) ? "s" : "k";
    return <span key={i} className={`t-${kind}`}>{part}</span>;
  });
}

export function CodeBlock({ code }: { code: string }) {
  return <pre className="code"><code>{highlight(code)}</code></pre>;
}

export function CodeTabs({ tabs }: { tabs: { label: string; note: string; code: string }[] }) {
  const [at, setAt] = useState(0);
  return (
    <div className="tabs">
      <div className="tab-list" role="tablist" aria-label="Code samples">
        {tabs.map((tab, i) => (
          <button key={tab.label} role="tab" id={`tab-${i}`} aria-selected={i === at} aria-controls={`panel-${i}`} tabIndex={i === at ? 0 : -1}
            onClick={() => setAt(i)}
            onKeyDown={(e) => {
              const d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
              if (!d) return;
              const next = (at + d + tabs.length) % tabs.length;
              setAt(next);
              document.getElementById(`tab-${next}`)?.focus();
            }}>
            {tab.label}
          </button>
        ))}
      </div>
      {tabs.map((tab, i) => (
        <div key={tab.label} role="tabpanel" id={`panel-${i}`} aria-labelledby={`tab-${i}`} hidden={i !== at}>
          <p className="tab-note">{tab.note}</p>
          <CodeBlock code={tab.code} />
        </div>
      ))}
    </div>
  );
}
