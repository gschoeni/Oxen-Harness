// Settings → Updates: the version running, the newest release, and the one
// button that installs it. Restarting stops any chat mid-turn, so with
// chats running the button asks once more before it goes.

import { useState } from "react";
import { ArrowUpCircle, CheckCircle2, ExternalLink, RotateCw, Terminal } from "lucide-react";
import { Button } from "../../components/ui";
import { Markdown } from "../../components/ui/Markdown";
import { openExternal } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { releaseDate, useUpdate } from "./update";
import "./update.css";

export function UpdatesPage() {
  const { status, checking, installing, error, check, install } = useUpdate();
  const running = useStore((s) => Object.values(s.runStatus).filter((r) => r === "running").length);
  const [confirming, setConfirming] = useState(false);

  function startInstall() {
    if (running > 0 && !confirming) {
      setConfirming(true);
      return;
    }
    setConfirming(false);
    void install();
  }

  const released = releaseDate(status?.published_at ?? null);
  const checked = status?.checked_at ? new Date(status.checked_at * 1000).toLocaleString() : null;

  return (
    <div className="settings-page">
      <section className={`update-card${status?.available ? " available" : ""}`} aria-label="Version">
        <div className="update-card-icon" aria-hidden="true">
          {status?.available ? <ArrowUpCircle size={22} /> : <CheckCircle2 size={22} />}
        </div>
        <div className="update-card-body">
          <div className="update-card-title">
            {status === null
              ? "Checking for updates…"
              : status.available
                ? `Version ${status.latest} is available`
                : "You're on the latest version"}
          </div>
          <dl className="update-versions">
            <div>
              <dt>Installed</dt>
              <dd>v{status?.current ?? "…"}</dd>
            </div>
            <div>
              <dt>Latest</dt>
              <dd>
                {status?.latest ? `v${status.latest}` : "—"}
                {released && <span className="update-date"> · {released}</span>}
              </dd>
            </div>
          </dl>
          {confirming && (
            <p className="update-warning" role="alert">
              {running === 1 ? "A chat is" : `${running} chats are`} still running. Restarting stops{" "}
              {running === 1 ? "it" : "them"} mid-turn.
            </p>
          )}
          <div className="update-actions">
            {status?.available && status.installable && (
              <Button variant="primary" onClick={startInstall} disabled={installing}>
                {installing ? "Installing…" : confirming ? "Restart anyway" : "Update & restart"}
              </Button>
            )}
            {status?.available && !status.installable && status.release_url && (
              <Button variant="primary" onClick={() => void openExternal(status.release_url ?? undefined)}>
                <ExternalLink size={14} /> Download v{status.latest}
              </Button>
            )}
            {confirming && (
              <Button variant="ghost" onClick={() => setConfirming(false)}>
                Not now
              </Button>
            )}
            {!confirming && (
              <Button variant="outline" onClick={() => void check(true)} disabled={checking || installing}>
                <RotateCw size={14} className={checking ? "update-spin" : undefined} />
                {checking ? "Checking…" : "Check now"}
              </Button>
            )}
          </div>
          {error && (
            <p className="update-error" role="alert">
              {error}
            </p>
          )}
          {checked && <p className="hint update-checked">Last checked {checked}</p>}
        </div>
      </section>

      {status?.notes && (
        <section className="settings-section">
          <div className="settings-label">
            What's new in v{status.latest}
            {status.release_url && (
              <button className="update-link" onClick={() => void openExternal(status.release_url ?? undefined)}>
                on GitHub <ExternalLink size={11} />
              </button>
            )}
          </div>
          <div className="update-notes">
            <Markdown text={status.notes} />
          </div>
        </section>
      )}

      <section className="settings-section">
        <div className="settings-label">The command line</div>
        <p className="hint update-cli">
          <Terminal size={14} aria-hidden="true" /> The <code>oxen-harness</code> CLI updates itself too: run{" "}
          <code>oxen-harness update</code>, or <code>/update</code> inside a session.
        </p>
      </section>
    </div>
  );
}
