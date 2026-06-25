import { Check, Cpu, Eye, EyeOff, Moon, Palette, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { Button, Modal } from "../../components/ui";
import { getConnection, setConnection } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import "./settings.css";

export function Settings() {
  const session = useStore((s) => s.session);
  const theme = useStore((s) => s.theme);
  const mode = useStore((s) => s.mode);
  const toggleMode = useStore((s) => s.toggleMode);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const setModelsOpen = useStore((s) => s.setModelsOpen);
  const setThemesOpen = useStore((s) => s.setThemesOpen);

  return (
    <Modal title="Settings" onClose={() => setSettingsOpen(false)}>
      <div className="settings-body">
        <ConnectionSettings />

        <section className="settings-section">
          <div className="settings-label">Session</div>
          <div className="meta">
            <Row label="model" value={session?.model ?? "—"} />
            <Row label="workspace" value={session?.workspace ?? "—"} title={session?.workspace} />
            <Row label="session" value={session ? session.session_id.slice(0, 8) : "—"} />
            <Row label="theme" value={theme?.meta.name ?? "—"} />
          </div>
          <p className="hint">
            The agent can read, write, and search files, run shell commands, and use
            git — scoped to the workspace above.
          </p>
        </section>

        <section className="settings-section">
          <div className="settings-label">Preferences</div>
          <nav className="settings-nav">
            <button className="nav-btn" onClick={() => setModelsOpen(true)}>
              <Cpu size={18} />
              <span>Local models</span>
            </button>
            <button className="nav-btn" onClick={() => setThemesOpen(true)}>
              <Palette size={18} />
              <span>Theme</span>
            </button>
            <button className="nav-btn" onClick={toggleMode}>
              {mode === "light" ? <Sun size={18} /> : <Moon size={18} />}
              <span>{mode === "light" ? "Light" : "Dark"}</span>
              <span className="nav-trail">toggle</span>
            </button>
          </nav>
        </section>
      </div>
    </Modal>
  );
}

/** Editable Oxen API key + host. Loads the saved values on open and rebuilds
 *  the agent on save (which starts a fresh session on the new endpoint). */
function ConnectionSettings() {
  const adoptSession = useStore((s) => s.adoptSession);
  const refreshHistory = useStore((s) => s.refreshHistory);

  const [host, setHost] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [braveKey, setBraveKey] = useState("");
  const [defaultHost, setDefaultHost] = useState("hub.oxen.ai");
  const [envKey, setEnvKey] = useState(false);
  const [revealKey, setRevealKey] = useState(false);
  const [revealBrave, setRevealBrave] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<{ ok: boolean; msg: string } | null>(null);

  useEffect(() => {
    getConnection()
      .then((c) => {
        setHost(c.host);
        setApiKey(c.api_key);
        setBraveKey(c.brave_api_key);
        setDefaultHost(c.default_host);
        setEnvKey(c.env_key_available);
      })
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);

  async function save() {
    setSaving(true);
    setStatus(null);
    try {
      const info = await setConnection(host.trim(), apiKey.trim(), braveKey.trim());
      adoptSession(info);
      refreshHistory();
      setStatus({ ok: true, msg: "Saved — started a fresh chat on the new settings." });
    } catch (e) {
      setStatus({ ok: false, msg: String(e) });
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-label">Oxen connection</div>
      <div className="fields">
        <label className="field">
          <span className="field-name">Host</span>
          <input
            className="field-input"
            placeholder={defaultHost}
            value={host}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => setHost(e.target.value)}
          />
        </label>
        <label className="field">
          <span className="field-name">API key</span>
          <div className="field-with-action">
            <input
              className="field-input"
              type={revealKey ? "text" : "password"}
              placeholder={envKey ? "Using OXEN_API_KEY / oxen login" : "sk-…"}
              value={apiKey}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setApiKey(e.target.value)}
            />
            <button
              type="button"
              className="field-action"
              aria-label={revealKey ? "Hide API key" : "Show API key"}
              title={revealKey ? "Hide" : "Show"}
              onClick={() => setRevealKey((r) => !r)}
            >
              {revealKey ? <EyeOff size={16} /> : <Eye size={16} />}
            </button>
          </div>
        </label>
        <label className="field">
          <span className="field-name">Brave Search API key</span>
          <div className="field-with-action">
            <input
              className="field-input"
              type={revealBrave ? "text" : "password"}
              placeholder="Enables web_search — get one at brave.com/search/api"
              value={braveKey}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setBraveKey(e.target.value)}
            />
            <button
              type="button"
              className="field-action"
              aria-label={revealBrave ? "Hide Brave key" : "Show Brave key"}
              title={revealBrave ? "Hide" : "Show"}
              onClick={() => setRevealBrave((r) => !r)}
            >
              {revealBrave ? <EyeOff size={16} /> : <Eye size={16} />}
            </button>
          </div>
        </label>
      </div>
      <div className="settings-actions">
        <Button variant="primary" size="sm" onClick={save} disabled={saving || !loaded}>
          {saving ? "Saving…" : "Save connection"}
        </Button>
        {status && (
          <span className={`save-status ${status.ok ? "ok" : "err"}`}>
            {status.ok && <Check size={14} />}
            {status.msg}
          </span>
        )}
      </div>
      <p className="hint">
        Point the agent at a different Oxen endpoint, paste an API key, or add a Brave
        Search key to enable web search. Leave a field blank to fall back to your{" "}
        <code>OXEN_*</code> / <code>BRAVE_API_KEY</code> environment or <code>oxen</code> CLI
        login. Saving starts a new chat.
      </p>
    </section>
  );
}

function Row({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="meta-row">
      <span className="meta-key">{label}</span>
      <span className="meta-val" title={title}>
        {value}
      </span>
    </div>
  );
}
