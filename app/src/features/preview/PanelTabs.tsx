// The right panel's Preview ⇄ Canvas switcher, shown in both panes' headers
// whenever the current chat has BOTH a live preview and a canvas document —
// with only one surface, the panel needs no tabs.

import { useStore } from "../../lib/store";

export function PanelTabs({ active }: { active: "preview" | "canvas" }) {
  const both = useStore((s) => {
    const id = s.session?.session_id;
    if (!id) return false;
    // A closed pane still counts as available — picking the Preview tab
    // reopens it (see `setRightTab`), which is what the user means.
    const previewAvailable = !!s.previews[id];
    const canvasId = s.activeCanvas[id];
    const canvasAvailable =
      !!s.canvasWriting[id] || (!!canvasId && !!s.canvases[id]?.some((d) => d.id === canvasId));
    return previewAvailable && canvasAvailable;
  });
  const setRightTab = useStore((s) => s.setRightTab);

  if (!both) return null;
  return (
    <div className="panel-tabs" role="tablist" aria-label="Panel view">
      {(["preview", "canvas"] as const).map((tab) => (
        <button
          key={tab}
          role="tab"
          aria-selected={tab === active}
          className={`panel-tab${tab === active ? " active" : ""}`}
          onClick={() => setRightTab(tab)}
        >
          {tab === "preview" ? "Preview" : "Canvas"}
        </button>
      ))}
    </div>
  );
}
