// The window's three panels — Files, Agent, Work — as one segmented switch in
// the title bar. Each segment shows or hides its panel; the last one showing
// can't be switched off, so the window is never blank. A hidden panel takes
// no space at all (no rail), so this is the one place to bring it back.

import { Bot, FolderTree, NotebookPen } from "lucide-react";
import type { ReactNode } from "react";
import { useStore } from "../../lib/store";
import { PANELS, togglePanel, usePanelShown, type Panel } from "./docks";

const SEGMENTS: Record<Panel, { label: string; icon: ReactNode; shortcut?: string }> = {
  left: { label: "Files", icon: <FolderTree size={14} />, shortcut: "⌘B" },
  chat: { label: "Agent", icon: <Bot size={14} /> },
  right: { label: "Work", icon: <NotebookPen size={14} />, shortcut: "⌘⌥B" },
};

export function PanelSwitch() {
  const shown = [usePanelShown("left"), usePanelShown("chat"), usePanelShown("right")];
  const hasSession = useStore((s) => !!s.session);
  const showing = shown.filter(Boolean).length;
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
            aria-pressed={on}
            disabled={last || (panel === "right" && !hasSession)}
            title={last ? `${label} is the only panel showing` : `${action} ${label}${shortcut ? ` (${shortcut})` : ""}`}
            onClick={() => togglePanel(panel)}
          >
            {icon}
            <span>{label}</span>
          </button>
        );
      })}
    </div>
  );
}
