// The two layout columns. Workbench owns view registration and navigation.
import type { ReactNode } from "react";
import { CHAT_MIN_FIT, FILES_DEFAULT_WIDTH, WORK_VIEW_DEFAULT_WIDTH, WORK_VIEW_MIN_WIDTH } from "./layout";
import { FolderTree, NotebookPen } from "lucide-react";
import { useStore } from "../../lib/store";
import { Workbench } from "../workbench/Workbench";
import { availableTarget } from "../workbench/registry";
import { FilesPanel } from "../files/FilesPanel";
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

/** Has this chat put something in the work view — a file, the gallery, a
 *  preview — or explicitly opened the empty pane from its rail? Until then
 *  the right column stays a rail: present, never an empty panel. */
type StoreState = ReturnType<typeof useStore.getState>;
function workViewOpened(s: Pick<StoreState, "session" | "rightTab" | "workContexts">): boolean {
  const session = s.session?.session_id;
  if (!session || !s.rightTab[session]) return false;
  if (s.rightTab[session] === "welcome") return true;
  const target = s.workContexts[session]?.current;
  return !!target && availableTarget(target).view !== "welcome";
}

/** Hidden: by the user, or (on the right) because this chat has nothing in
 *  the work view yet. The layout solver and the column agree on this one
 *  answer, so the chat is never laid out against a width the column isn't
 *  using. */
export function useDockCollapsed(side: DockSide): boolean {
  return useStore((s) => !panelShown(s, side));
}

/** The window's three panels, as the title bar's switch names them. The
 *  chat's hidden flag rides in `dockCollapsed` beside the docks' so all three
 *  persist as one layout. */
export type Panel = DockSide | "chat";
export const PANELS: Panel[] = ["left", "chat", "right"];

export function panelShown(
  s: Pick<StoreState, "session" | "rightTab" | "workContexts" | "dockCollapsed">,
  panel: Panel,
): boolean {
  if (s.dockCollapsed[panel]) return false;
  return panel !== "right" || workViewOpened(s);
}

export function usePanelShown(panel: Panel): boolean {
  return useStore((s) => panelShown(s, panel));
}

/** Show or hide one panel. The last one showing stays: the window is never
 *  blank. Showing a dock makes room honestly — if the window can't fit it at
 *  its wanted width beside the chat, it takes only its minimum and the
 *  other dock hides, so the click is never one the solver folds right back. */
export function setPanelShown(panel: Panel, shown: boolean) {
  const s = useStore.getState();
  if (!shown) {
    const others = PANELS.filter((p) => p !== panel && panelShown(s, p));
    if (others.length > 0) s.setDockCollapsed(panel, true);
    return;
  }
  if (panel === "right") openEmptyWorkView();
  if (panel !== "chat" && panelShown(s, "chat")) {
    const other: DockSide = panel === "left" ? "right" : "left";
    const spec = docksOnSide(panel)[0];
    const want = s.dockWidths[panel] ?? spec?.defaultWidth ?? 0;
    const otherWidth = panelShown(s, other) ? (s.dockWidths[other] ?? 0) : 0;
    if (window.innerWidth - otherWidth - want < CHAT_MIN_FIT) {
      if (spec) s.setDockWidth(panel, spec.minWidth);
      if (window.innerWidth - otherWidth - (spec?.minWidth ?? 0) < CHAT_MIN_FIT) {
        s.setDockCollapsed(other, true);
      }
    }
  }
  s.setDockCollapsed(panel, false);
}

export function togglePanel(panel: Panel) {
  setPanelShown(panel, !panelShown(useStore.getState(), panel));
}

/** Expand the right column for a chat that has not opened it: saved history
 *  (a file from a previous run) comes back where it was; otherwise the empty
 *  pane itself becomes what this chat opened. */
export function openEmptyWorkView() {
  const s = useStore.getState();
  if (!s.session || workViewOpened(s)) return;
  const session = s.session.session_id;
  const saved = s.workContexts[session]?.current;
  s.openWorkView(session, saved && availableTarget(saved).view !== "welcome" ? saved : { view: "welcome" });
}

export const DOCKS: DockSpec[] = [
  { id: "files", side: "left", title: "Files", icon: <FolderTree size={15}/>, defaultWidth: FILES_DEFAULT_WIDTH, minWidth: 216, useAvailable: () => true, render: ({onResizeStart}) => <FilesPanel onResizeStart={onResizeStart}/> },
  { id: "workbench", side: "right", title: "Work view", icon: <NotebookPen size={15}/>, defaultWidth: WORK_VIEW_DEFAULT_WIDTH, minWidth: WORK_VIEW_MIN_WIDTH, useAvailable: () => useStore((s) => !!s.session), render: ({onResizeStart}) => <Workbench onResizeStart={onResizeStart}/> },
];

export const docksOnSide = (side: DockSide) => DOCKS.filter((d) => d.side === side);

/** The docks on `side` that have something to show right now, in registry
 *  order. Calls every dock's `useAvailable` unconditionally, so the hook order
 *  is stable regardless of what's open. */
export function useAvailableDocks(side: DockSide): DockSpec[] {
  const flags = DOCKS.map((dock) => dock.useAvailable());
  return DOCKS.filter((dock, i) => dock.side === side && flags[i]);
}
