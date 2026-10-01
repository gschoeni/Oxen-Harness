import { useEffect, useState, type ReactNode } from "react";
import { ArrowRight, Eye, EyeOff, KeyRound } from "lucide-react";
import { Button } from "../../components/ui";
import { getConnection } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { Item } from "./thread";
import "./apikey.css";

type ApiKeyItem = Extract<Item, { kind: "apikey" }>;

/** Inline recovery for a turn that failed authentication (a 401): a compact card,
 *  shown where the reply would be, that takes an Oxen API key and — on save —
 *  authenticates the running chat and retries the failed turn, so the
 *  conversation continues without a trip to Settings or a fresh session. */
export function ApiKeyPrompt({ item }: { item: ApiKeyItem }) {
  const submitApiKey = useStore((s) => s.submitApiKey);
  return (
    <ApiKeyCard
      why={(host) => (
        <>
          That request wasn’t authorized — no API key is set for {host}. Paste one to continue
          this chat.
        </>
      )}
      action="Save & retry"
      // On success the card is removed from the thread, unmounting the form.
      onSave={(session, key) => submitApiKey(session, item.id, key)}
      autoFocus
    />
  );
}

/** The same card before anything was sent: an empty chat with no key shows it
 *  where the example prompts would be, so the first message isn't spent
 *  discovering a 401. Saving it brings the examples back. */
export function ApiKeyWelcome() {
  const saveApiKey = useStore((s) => s.saveApiKey);
  return (
    <ApiKeyCard
      className="welcome"
      why={(host) => (
        <>
          No API key is set for {host} yet. Paste one to start chatting — or pick a local model
          below, which needs none.
        </>
      )}
      action="Save key"
      onSave={saveApiKey}
    />
  );
}

function ApiKeyCard({
  why,
  action,
  onSave,
  className = "",
  autoFocus = false,
}: {
  /** The line under the title, given the endpoint the key is for. */
  why: (host: ReactNode) => ReactNode;
  action: string;
  onSave: (session: string, key: string) => Promise<void>;
  className?: string;
  /** Only the recovery card takes focus; up front, the composer keeps it. */
  autoFocus?: boolean;
}) {
  const sessionId = useStore((s) => s.session?.session_id);
  const openSettings = useStore((s) => s.openSettings);

  const [key, setKey] = useState("");
  const [reveal, setReveal] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The endpoint this key will authenticate against — shown so it's clear which
  // host (the default hub, or a custom/self-hosted one) is being connected.
  const [host, setHost] = useState("");

  useEffect(() => {
    getConnection()
      .then((c) => setHost(c.host || c.default_host))
      .catch(() => {});
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = key.trim();
    if (!trimmed || !sessionId || saving) return;
    setSaving(true);
    setError(null);
    try {
      await onSave(sessionId, trimmed);
    } catch (err) {
      setError(String(err));
    } finally {
      // A save that worked normally unmounts the card; if it is still here,
      // it must not stay locked on "Saving…".
      setSaving(false);
    }
  }

  return (
    <div className={`apikey-card ${className}`.trim()}>
      <div className="apikey-head">
        <span className="apikey-icon">
          <KeyRound size={15} />
        </span>
        <div className="apikey-head-text">
          <div className="apikey-title">Connect your Oxen account</div>
          <div className="apikey-sub">
            {why(<code className="apikey-host">{host || "your Oxen endpoint"}</code>)}
          </div>
        </div>
      </div>

      <form className="apikey-form" onSubmit={submit}>
        <div className="apikey-input-wrap">
          <input
            className="apikey-input"
            type={reveal ? "text" : "password"}
            placeholder="Oxen API key (sk-…)"
            value={key}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            autoFocus={autoFocus}
            disabled={saving}
            onChange={(e) => setKey(e.target.value)}
          />
          <button
            type="button"
            className="apikey-reveal"
            aria-label={reveal ? "Hide API key" : "Show API key"}
            title={reveal ? "Hide" : "Show"}
            onClick={() => setReveal((r) => !r)}
          >
            {reveal ? <EyeOff size={15} /> : <Eye size={15} />}
          </button>
        </div>
        <Button type="submit" variant="primary" size="sm" disabled={!key.trim() || saving}>
          {saving ? "Saving…" : action}
          {!saving && <ArrowRight size={15} />}
        </Button>
      </form>

      {error && <div className="apikey-error">{error}</div>}

      <div className="apikey-foot">
        Stored locally in <code>~/.oxen-harness/.env</code>. Get a key at{" "}
        <code>{host || "hub.oxen.ai"}/settings</code>, or open{" "}
        <button className="apikey-link" type="button" onClick={() => openSettings("connection")}>
          Connection settings
        </button>
        .
      </div>
    </div>
  );
}
