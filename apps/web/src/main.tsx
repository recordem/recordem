import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { completeSignIn, signedIn, signIn } from "./auth";
import { Home } from "./Home";
import { MeetingPage } from "./Meeting";
import { navigate, usePath } from "./router";
import { Settings } from "./Settings";
import "./styles.css";

function Callback() {
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { completeSignIn().then((to) => navigate(to), (e: Error) => setError(e.message)); }, []);
  return <div className="center">{error ? <><p className="error">{error}</p><button onClick={() => signIn("/")}>Sign in again</button></> : <p className="muted">Signing in…</p>}</div>;
}

function App() {
  const path = usePath();
  if (path.startsWith("/callback")) return <Callback />;
  if (!signedIn()) {
    return (
      <div className="center">
        <h1 className="brand"><span className="dot" /> Recordem</h1>
        <p className="muted">Your meetings, transcribed and kept.</p>
        <button onClick={() => signIn()}>Sign in</button>
      </div>
    );
  }
  if (path.startsWith("/settings")) return <Settings />;
  const m = path.match(/^\/m\/([0-9a-f-]{36})/);
  return m ? <MeetingPage id={m[1]} /> : <Home />;
}

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
