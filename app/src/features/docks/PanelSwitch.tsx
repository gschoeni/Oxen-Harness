// The window's panels — Files, Agent, Work — plus the chat's data inspector
// (</>), as one icon-only segmented switch in the title bar. Each panel
// segment shows or hides its panel; the last one showing can't be switched
// off, so the window is never blank. A hidden panel takes no space at all (no
// rail), so this is the one place to bring it back. The inspector segment
// opens and closes the raw-transcript drawer over the current chat.

import { Bot, Code2, FolderTree, NotebookPen } from "lucide-react";
import type { ReactNode } from "react";
import { useStore } from "../../lib/store";
import { PANELS, togglePanel, usePanelShown, type Panel } from "./docks";

const SEGMENTS: Record<Panel, { label: string; icon: ReactNode; shortcut?: string }> = {
  left: { label: "Files", icon: <FolderTree strokeWidth={1.75} />, shortcut: "⌘B" },
  chat: { label: "Agent", icon: <Bot strokeWidth={1.75} /> },
  right: { label: "Work", icon: <NotebookPen strokeWidth={1.75} />, shortcut: "⌘⌥B" },
};

export function PanelSwitch() {
  const shown = [usePanelShown("left"), usePanelShown("chat"), usePanelShown("right")];
  const sessionId = useStore((s) => s.session?.session_id);
  const inspecting = useStore((s) => !!s.inspector);
  const showing = shown.filter(Boolean).length;
  const toggleInspector = () => {
    const s = useStore.getState();
    if (s.inspector) s.closeInspector();
    else if (sessionId) s.openInspector(sessionId);
  };
  return (
    <div className="panel-switch" role="group" aria-label="Panels">
      {PANELS.map((panel, i) => {
        const { label, icon, shortcut } = SEGMENTS[panel];
        const on = shown[i];
        const last = on && showing === 1;
        const action = on ? "Hide" : "Show";
        return (
          <button
            key={panel}
            type="button"
            className={`panel-switch-seg${on ? " on" : ""}`}
            aria-label={label}
            aria-pressed={on}
            disabled={last || (panel === "right" && !sessionId)}
            title={last ? `${label} is the only panel showing` : `${action} ${label}${shortcut ? ` (${shortcut})` : ""}`}
            onClick={() => togglePanel(panel)}
          >
            {icon}
          </button>
        );
      })}
      <span className="panel-switch-sep" aria-hidden="true" />
      <button
        type="button"
        className={`panel-switch-seg${inspecting ? " on" : ""}`}
        aria-label="Inspector"
        aria-pressed={inspecting}
        disabled={!sessionId && !inspecting}
        title="Inspect this chat — the raw LLM inputs and outputs for this session"
        onClick={toggleInspector}
      >
        <Code2 strokeWidth={1.75} />
      </button>
    </div>
  );
}
