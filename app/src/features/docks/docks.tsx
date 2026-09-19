// The two layout columns. Workbench owns view registration and navigation.
import type { ReactNode } from "react";
import { FolderTree, NotebookPen } from "lucide-react";
import { useStore } from "../../lib/store";
import { Workbench } from "../workbench/Workbench";
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

export const DOCKS: DockSpec[] = [
  { id: "files", side: "left", title: "Files", icon: <FolderTree size={16}/>, defaultWidth: 280, minWidth: 216, useAvailable: () => true, render: ({onResizeStart}) => <FilesPanel onResizeStart={onResizeStart}/> },
  { id: "workbench", side: "right", title: "Work view", icon: <NotebookPen size={16}/>, defaultWidth: 680, minWidth: 340, useAvailable: () => useStore(s => !!s.session), render: ({onResizeStart}) => <Workbench onResizeStart={onResizeStart}/> },
];

export const docksOnSide = (side: DockSide) => DOCKS.filter((d) => d.side === side);

/** The docks on `side` that have something to show right now, in registry
 *  order. Calls every dock's `useAvailable` unconditionally, so the hook order
 *  is stable regardless of what's open. */
export function useAvailableDocks(side: DockSide): DockSpec[] {
  const flags = DOCKS.map((dock) => dock.useAvailable());
  return DOCKS.filter((dock, i) => dock.side === side && flags[i]);
}
