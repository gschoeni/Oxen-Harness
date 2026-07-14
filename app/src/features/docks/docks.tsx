// The dock registry: every side panel the app can show, in one list.
//
// A dock is a column that lives on the left or right of the chat. Docks on the
// same side share one column and appear as tabs when more than one has
// something to show; each side is independently resizable and collapsible, and
// those choices persist.
//
// ADDING A DOCK is this file plus your component:
//
//   {
//     id: "terminal",
//     side: "right",
//     title: "Terminal",
//     icon: <TerminalIcon size={15} />,
//     defaultWidth: 520,
//     minWidth: 320,
//     useAvailable: () => useStore((s) => !!s.terminal),   // has content?
//     render: () => <TerminalPanel />,
//   }
//
// Everything else — the column, the tab strip, the drag-resize, the collapse
// button, the persisted width, the keyboard shortcut — comes for free. The
// only rule: `useAvailable` must be a hook-safe selector (it runs every render
// for every dock).

import type { ReactNode } from "react";
import { Globe, MessagesSquare, NotebookPen } from "lucide-react";
import { useStore } from "../../lib/store";
import { BrandMark } from "../history/BrandMark";
import { Sidebar } from "../history/Sidebar";
import { Canvas } from "../canvas/Canvas";
import { Preview } from "../preview/Preview";

export type DockSide = "left" | "right";

export interface DockSpec {
  /** Stable id: persists the width/collapse state and names the active tab. */
  id: string;
  side: DockSide;
  /** Shown in the tab strip and the collapsed rail's tooltip. */
  title: string;
  /** Shown in the collapsed rail (and the tab strip when space is tight). */
  icon: ReactNode;
  defaultWidth: number;
  minWidth: number;
  /** Whether this dock currently has anything to show. */
  useAvailable: () => boolean;
  /** The dock's content. `onResizeStart` wires the column's drag handle. */
  render: (props: { onResizeStart?: (e: React.PointerEvent) => void }) => ReactNode;
  /** Optional mark pinned to the top of this side's collapsed rail, so the
   *  column keeps its identity (and its vertical rhythm) when collapsed —
   *  the app's logo stays put instead of vanishing. */
  railHeader?: () => ReactNode;
  /** Docks the user can't collapse away (the chat list is the app's spine —
   *  but it can still be collapsed to a rail; this is for future docks that
   *  must always render). */
  alwaysOpen?: boolean;
}

/** Does the current chat have a canvas document open (or one being written)? */
function useCanvasAvailable(): boolean {
  return useStore((s) => {
    const id = s.session?.session_id;
    if (!id) return false;
    if (s.canvasWriting[id]) return true;
    const active = s.activeCanvas[id];
    return !!active && !!s.canvases[id]?.some((d) => d.id === active);
  });
}

/** Does the current chat have a dev server worth showing (running, starting,
 *  or stopped-with-a-reason — the pane holds the Restart button)? */
function usePreviewAvailable(): boolean {
  return useStore((s) => {
    const id = s.session?.session_id;
    if (!id || s.previewClosed[id]) return false;
    return !!s.previews[id];
  });
}

export const DOCKS: DockSpec[] = [
  {
    id: "history",
    side: "left",
    title: "Chats",
    icon: <MessagesSquare size={16} />,
    defaultWidth: 272,
    minWidth: 208,
    useAvailable: () => true,
    render: ({ onResizeStart }) => <Sidebar onResizeStart={onResizeStart} />,
    railHeader: () => <BrandMark />,
  },
  {
    id: "preview",
    side: "right",
    title: "Preview",
    icon: <Globe size={16} />,
    defaultWidth: 480,
    minWidth: 320,
    useAvailable: usePreviewAvailable,
    render: ({ onResizeStart }) => <Preview onResizeStart={onResizeStart} />,
  },
  {
    id: "canvas",
    side: "right",
    title: "Canvas",
    icon: <NotebookPen size={16} />,
    defaultWidth: 480,
    minWidth: 320,
    useAvailable: useCanvasAvailable,
    render: ({ onResizeStart }) => <Canvas onResizeStart={onResizeStart} />,
  },
];

export const docksOnSide = (side: DockSide) => DOCKS.filter((d) => d.side === side);

/** The docks on `side` that have something to show right now, in registry
 *  order. Calls every dock's `useAvailable` unconditionally, so the hook order
 *  is stable regardless of what's open. */
export function useAvailableDocks(side: DockSide): DockSpec[] {
  const flags = DOCKS.map((dock) => dock.useAvailable());
  return DOCKS.filter((dock, i) => dock.side === side && flags[i]);
}
