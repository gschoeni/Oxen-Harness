import { Plus, Settings as SettingsIcon } from "lucide-react";
import { useStore } from "../../lib/store";
import { relativeTime } from "../../lib/format";
import type { SessionSummary } from "../../lib/types";
import "./sidebar.css";

export function Sidebar() {
  const theme = useStore((s) => s.theme);
  const sessions = useStore((s) => s.sessions);
  const session = useStore((s) => s.session);
  const runStatus = useStore((s) => s.runStatus);
  const startNewSession = useStore((s) => s.startNewSession);
  const resume = useStore((s) => s.resume);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);

  const icon = theme?.voice.prompt_icon || "🐂";
  const currentId = session?.session_id ?? null;

  // A brand-new active session isn't persisted with a title yet — pin it on top
  // so the user can always see which chat they're in.
  const rows: SessionSummary[] = [...sessions];
  if (currentId && !sessions.some((s) => s.id === currentId)) {
    rows.unshift({
      id: currentId,
      workspace: "",
      model: "",
      created_at: 0,
      title: null,
      message_count: 0,
    });
  }

  return (
    <aside className="sidebar">
      {/* Transparent strip over the traffic-light row so the empty space above
          the brand drags the window (overlay title bar). */}
      <div className="sidebar-titlebar" data-tauri-drag-region />
      <div className="brand" data-tauri-drag-region>
        <span className="brand-icon">{icon}</span>
        <span>oxen-harness</span>
      </div>

      <button className="new-chat" onClick={() => startNewSession()}>
        <Plus size={17} />
        New chat
      </button>

      <div className="history-head">Chats</div>
      <div className="history">
        {rows.length === 0 ? (
          <div className="history-empty">
            No chats yet — your conversations will appear here.
          </div>
        ) : (
          rows.map((s) => {
            const status = runStatus[s.id];
            return (
              <button
                key={s.id}
                className={`history-item ${s.id === currentId ? "active" : ""}`}
                onClick={() => resume(s.id)}
              >
                <span className="history-text">
                  <span className="history-title">{s.title?.trim() || "New chat"}</span>
                  <span className="history-sub">
                    {s.created_at ? relativeTime(s.created_at) : "Not started yet"}
                  </span>
                </span>
                {status === "running" ? (
                  <span className="chat-status running" title="Running" aria-label="Running">
                    <span className="run-dot" />
                  </span>
                ) : status === "unread" && s.id !== currentId ? (
                  // Never dot the chat you're already viewing (you've seen it).
                  <span
                    className="chat-status unread"
                    title="Done — unread"
                    aria-label="Done, unread"
                  />
                ) : null}
              </button>
            );
          })
        )}
      </div>

      <div className="sidebar-foot">
        <button className="foot-btn" onClick={() => setSettingsOpen(true)}>
          <SettingsIcon size={17} />
          Settings
        </button>
      </div>
    </aside>
  );
}
