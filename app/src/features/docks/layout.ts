// The column-fit solver: no panel may ever push another off the window.
//
// The app is three tracks — left docks | chat | right docks. Dock widths are
// fixed pixels the user chose; the chat takes what's left. When the window
// shrinks (or a dock opens wide), something has to yield, in this order:
//
//   1. the right column shrinks toward its dock's minimum, then the left;
//   2. the right column folds to its icon rail, then the left;
//   3. finally the chat itself folds to a bar with one "show the agent"
//      button — squeezed, never gone.
//
// The plan is derived per render from the window width — nothing here writes
// to the store, so widening the window restores exactly what the user had.

/** Width of a collapsed column's icon rail. */
export const RAIL_W = 52;
/** The chat width the solver defends: below this, columns start yielding. */
export const CHAT_MIN_FIT = 320;
/** Below this the chat folds to its own rail (a bar with an expand button). */
export const CHAT_RAIL_MIN = 240;

export interface ColumnInput {
  /** Does this side have any dock with content? */
  available: boolean;
  /** Did the user collapse this side to its rail? */
  collapsed: boolean;
  /** The user's chosen (or default) expanded width, px. */
  desired: number;
  /** The active dock's minimum usable width, px. */
  min: number;
}

export interface ColumnPlan {
  width: number;
  /** Render as an icon rail (user-collapsed or squeezed by the solver). */
  railed: boolean;
}

export interface LayoutPlan {
  left: ColumnPlan | null;
  right: ColumnPlan | null;
  /** The terminal squeeze: render the chat as a bar with an expand button. */
  chatRailed: boolean;
}

export function planColumns(
  windowWidth: number,
  left: ColumnInput,
  right: ColumnInput,
): LayoutPlan {
  const mk = (c: ColumnInput): ColumnPlan | null =>
    c.available ? { width: c.collapsed ? RAIL_W : c.desired, railed: c.collapsed } : null;
  const plan = { left: mk(left), right: mk(right) };
  const chat = () => windowWidth - (plan.left?.width ?? 0) - (plan.right?.width ?? 0);

  // The right column yields first: it holds reference material (editor,
  // preview, canvas); the left is the app's navigation; the chat is its spine.
  const order: Array<[ColumnPlan | null, ColumnInput]> = [
    [plan.right, right],
    [plan.left, left],
  ];
  for (const [column, input] of order) {
    if (!column || column.railed) continue;
    const deficit = CHAT_MIN_FIT - chat();
    if (deficit <= 0) break;
    column.width = Math.max(input.min, column.width - deficit);
  }
  for (const [column] of order) {
    if (!column || column.railed) continue;
    if (chat() >= CHAT_MIN_FIT) break;
    column.width = RAIL_W;
    column.railed = true;
  }
  return { ...plan, chatRailed: chat() < CHAT_RAIL_MIN };
}
