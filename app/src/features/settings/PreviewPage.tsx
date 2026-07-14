// Settings → Preview: the live-preview knobs and the running dev servers.
//
// Auto-verify is the trust dial for non-coders: with it on, the agent looks at
// what it built (screenshot + browser console) after each edit batch and fixes
// what it finds before saying "done". The servers list shows every chat's dev
// server with a stop button — the escape hatch when something is squatting on
// a port.

import { useCallback, useEffect, useState } from "react";
import { getPreviewPrefs, previewStatuses, previewStop, setPreviewAutoVerify } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import { ToolSwitch } from "../tools/ToolSwitch";
import type { PreviewStatus } from "../../lib/types";

export function PreviewPage() {
  // `null` until the saved value loads, so the switch never flashes the wrong
  // state (and a failed save reverts it rather than lying).
  const [autoVerify, setAutoVerify] = useState<boolean | null>(null);
  const [servers, setServers] = useState<[string, PreviewStatus][]>([]);
  const sessions = useStore((s) => s.sessions);
  // Servers start and stop from the chat while this page is open; the store
  // already receives every `preview://status`, so re-poll whenever it changes.
  const previews = useStore((s) => s.previews);

  const refresh = useCallback(() => {
    previewStatuses()
      .then(setServers)
      .catch(() => {});
  }, []);

  useEffect(() => {
    getPreviewPrefs()
      .then((p) => setAutoVerify(p.auto_verify))
      .catch(() => setAutoVerify(true));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, previews]);

  const toggleAutoVerify = (_name: string, next: boolean) => {
    const previous = autoVerify;
    setAutoVerify(next);
    setPreviewAutoVerify(next).catch(() => setAutoVerify(previous ?? true));
  };

  const titleFor = (sessionId: string) =>
    sessions.find((s) => s.id === sessionId)?.title?.trim() || "Untitled chat";

  const running = servers.filter(([, s]) => s.phase === "ready" || s.phase === "starting");

  return (
    <div className="settings-page">
      <section className="settings-section">
        <div className="settings-label">Checking its own work</div>
        <div className="preview-verify-row">
          <div className="preview-verify-text">
            <span className="preview-verify-title">Auto-verify changes in the preview</span>
            <p className="hint">
              After editing your app, the agent takes a screenshot and checks for browser errors,
              fixing problems before telling you it's done. Uses some extra tokens; applies to new
              chats.
            </p>
          </div>
          <ToolSwitch name="auto-verify" enabled={autoVerify ?? true} onToggle={toggleAutoVerify} />
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-label">Running dev servers</div>
        {running.length === 0 ? (
          <p className="hint">
            None right now. Ask a chat to build or run a web app and its server will appear here
            (and in a live Preview panel next to that chat).
          </p>
        ) : (
          <div className="meta">
            {running.map(([sessionId, s]) => (
              <div className="meta-row preview-server-row" key={sessionId}>
                <span className="meta-key" title={s.command}>
                  {titleFor(sessionId)} · {s.name}
                </span>
                <span className="preview-server-url">{s.url ?? "starting…"}</span>
                <button
                  className="preview-server-stop"
                  onClick={() => previewStop(sessionId).then(refresh).catch(() => {})}
                >
                  Stop
                </button>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
