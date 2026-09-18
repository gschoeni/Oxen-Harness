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
import { Compass, FileCode2, FolderTree, Globe, Images, NotebookPen } from "lucide-react";
import { useStore } from "../../lib/store";
import { Canvas } from "../canvas/Canvas";
import { Preview } from "../preview/Preview";
import { Browser } from "../browser/Browser";
import { FilesPanel } from "../files/FilesPanel";
import { EditorPane } from "../files/EditorPane";
import { GalleryPanel } from "../media/GalleryPanel";
import { ProjectsNav } from "../projects/ProjectsNav";

export type DockSide = "left" | "right";

/** Column chrome rendered above the tab strip, per side. The left column
 *  carries the Home back-link — top-level navigation that outlives whichever
 *  dock is active below it. */
export function ColumnNav({ side }: { side: DockSide }) {
  return side === "left" ? <ProjectsNav /> : null;
}

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
  /** Docks the user can't collapse away (none today — every dock can still
   *  fold to a rail; this is for future docks that must always render). */
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

/** Does the current chat have files open in the Editor/viewer pane? */
function useEditorAvailable(): boolean {
  return useStore((s) => {
    const id = s.session?.session_id;
    return !!id && (s.editorTabs[id]?.tabs.length ?? 0) > 0;
  });
}

/** Does the current chat's project have generations to show (or did the
 *  user ask for the Gallery explicitly)? */
function useGalleryAvailable(): boolean {
  return useStore((s) => {
    const id = s.session?.session_id;
    const root = s.session?.workspace;
    if (!id || !root) return false;
    return (s.media[root]?.length ?? 0) > 0 || s.rightTab[id] === "gallery";
  });
}

export const DOCKS: DockSpec[] = [
  {
    // The left column's spine: always present, so the Home link, the collapse
    // toggle, and ⌘B are there before a project is open (the panel shows a
    // hint until one is). Chats aren't a dock — they're the tab strip above
    // the chat, with the history a click away (see features/tabs).
    id: "files",
    side: "left",
    title: "Files",
    icon: <FolderTree size={16} />,
    defaultWidth: 280,
    minWidth: 216,
    useAvailable: () => true,
    render: ({ onResizeStart }) => <FilesPanel onResizeStart={onResizeStart} />,
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
  {
    id: "editor",
    side: "right",
    title: "Editor",
    icon: <FileCode2 size={16} />,
    defaultWidth: 560,
    /* Low enough that rail + editor + the solver's chat minimum fit in the
       window's own minimum width — expanding from a rail must never be a
       dead click (see DockColumn's `expand`). */
    minWidth: 340,
    useAvailable: useEditorAvailable,
    render: ({ onResizeStart }) => <EditorPane onResizeStart={onResizeStart} />,
  },
  {
    id: "gallery",
    side: "right",
    title: "Gallery",
    icon: <Images size={16} />,
    defaultWidth: 520,
    minWidth: 340,
    useAvailable: useGalleryAvailable,
    render: ({ onResizeStart }) => <GalleryPanel onResizeStart={onResizeStart} />,
  },
  {
    id: "browser",
    side: "right",
    title: "Browser",
    icon: <Compass size={16} />,
    defaultWidth: 480,
    minWidth: 320,
    useAvailable: () => useStore((s) => !!s.browserUrl),
    render: ({ onResizeStart }) => <Browser onResizeStart={onResizeStart} />,
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
