# Chat tabs: replace the left-hand chat list with a tab strip + history modal

Status: shipped 2026-09-17, all five phases, then a rename feature (double-click, persisted `title` session_state) and a hardening pass (drag reorder, arrow-key focus, boot-prune only after a real history load, failure notices, per-chat question card, search without full paths, pure helpers in `lib/chatTabs.ts`), with the recommended answer to every open question below (per-project tabs; closing keeps the agent running and badges the history; accent/warning/green/danger; Settings gear in the title bar; client-side search; strip inside the chat column; ⌘T/⌘W/⌘K/⌃Tab/⌘1–9, with the native menu's Close Window item removed so ⌘W reaches the strip).

## What exists today

- **Left column is the dock registry** (`app/src/features/docks/docks.tsx`). Two left docks:
  `history` (the `Sidebar`, always available) and `files` (available once a workspace is set).
  `DockColumn` shows a Chats/Files tab strip only when both have content. `ColumnNav` renders
  the `ProjectsNav` "Home" back-link above whichever dock is active.
- **`Sidebar` (`features/history/Sidebar.tsx`) owns five things** that must find new homes:
  project title + the left `DockToggle`, the "New chat" button, the sectioned chat list
  (Needs you / Chats / Settled, driven by `useBoard()` + `ledger.ts` `needsUser/needLabel/needRank`),
  the delete-confirm `Modal`, and the Settings footer button.
- **Store already supports many concurrent chats.** `session` is the visible chat, `sessions` is
  every persisted `SessionSummary` (all projects), `runStatus[id]` is `"running" | "unread"`,
  `resume(id)` swaps the view (rebuilding the thread from the transcript when it was evicted),
  `startNewSession()` mints a fresh one, `removeSessions()` purges. Thread caches are capped
  (`MAX_CACHED_THREADS = 4`, `MAX_PROTECTED_RUNNING = 6`) so open tabs must NOT imply resident
  threads. `resume` is the single entry point every surface uses (Ledger `useOpenThread`,
  `cliOpen` `session:` handoff, sidebar rows), which is where tab registration belongs.
- **"Needs you" signals available per session:** `approvals[id]` (permission prompt pending),
  the ledger `Thread` (`stuck`, `need` ∈ dangling/finished/plan-open/going-cold, `state`), and
  `runStatus[id] === "unread"`. Gap: the clarifying-question payload (`store.question`) is global,
  not session-tagged, so a background chat waiting on a question can't be flagged. Needs a
  `session` field on the question event (small backend/bindings change).
- **Persistence** goes through `lib/uiState.ts` (`~/.oxen-harness/ui.json`), not localStorage.
- **Style tokens** (`styles/tokens.css`, `styles/pixel.css`): `--accent` (running), `--warning`
  (needs you), `--green`/`--green-soft`, `--danger`/`--danger-soft`, frame tokens
  (`--frame-border-w/radius/shadow`), display font + label tokens for caps headings,
  `.dock-tab.active` pattern (bg `--bg`, separator border, `--shadow-1`). The sidebar's
  `.run-dot` breathing animation and `.chat-status.needy/.unread` dots are reusable as-is.
- **Title bar** (`TitleBar.tsx`, 38px, native drag region) holds: running count → Ledger,
  project home, arcade, inspector. Only keyboard shortcuts in the app today: ⌘B / ⌘⌥B.
- **Search**: no backend session search. `listSessions()` already returns every summary
  (title, model, workspace, created_at, message_count) so client-side filtering is free.
  Full-text over messages would need a new `/v1/sessions/search` endpoint.
- **Tests to touch**: `Sidebar.test.tsx` (17 cases, to be replaced), `DockColumn.test.tsx`
  ("shows tabs only when a side has more than one dock" relies on the left Chats+Files pair),
  `store.test.ts`, `App.test.tsx`.

## Target design

### Layout

```
┌ TitleBar (drag region) ─────────────────────────────────── ⚙ ⏱ ▣ 🎮 </> ┐
├ Home ◂ ───────┬─ [● Fix parser ×] [◐ Refactor ×] [New chat ×]  +  🕘 ──────┬────────┐
│ Files         │  TrailStrip / Plan / messages …                             │ right  │
│ (tree,        │                                                             │ docks  │
│  changes)     │  Composer                                                   │        │
└───────────────┴─────────────────────────────────────────────────────────────┴────────┘
```

- The tab strip lives at the top of the chat column (`<main className="chat">`), above
  `TrailStrip`. Left/right docks keep their own headers, so nothing else shifts.
- Left column defaults to Files. The `history` dock entry is removed from the registry; the
  `files` dock becomes always-available and renders a short empty state when no workspace is
  open ("No project open. Pick one from Home."), so the column, ⌘B, and the Home link survive.
  The left `DockToggle` moves into the Files header (`.ft-head`, beside the project name).
- Settings moves to a gear button in the title-bar actions. Home stays as the column nav.

### Tab strip (`features/tabs/ChatTabs.tsx` + `tabs.css`)

- One tab per open chat of the current project. Content: status dot, title (ellipsis,
  min 120px / max 220px, "New chat" until the first turn lands), close × (visible on hover and
  on the active tab). Middle-click closes. Overflow scrolls horizontally with the active tab
  kept in view. `+` at the end starts a chat (reuses the sidebar's "already on a fresh chat"
  guard so it can't pile up orphans). A history button (lucide `History`) opens the modal and
  wears a warning badge with the count of *closed* chats that need you.
- Right-click menu (existing `Menu` component): Close, Close others, Close to the right,
  Copy session id, Delete chat… (same confirm modal).
- Active tab mirrors `.dock-tab.active`. Tabs are framed in the theme's frame tokens so the
  pixel/retro themes get their hard-edged shadow automatically.

### Tab states and colors

Derived by one pure function `tabStatus(id, store)` in `features/tabs/tabStatus.ts`
(unit-tested), combining `runStatus`, `approvals`, the ledger thread, and the pending question:

| state | meaning | color | indicator |
|---|---|---|---|
| idle | nothing owed | `--text-secondary` | none |
| running | turn/fleet in flight | `--accent` | breathing `.run-dot` (reused) |
| needs you | approval or question pending, agent parked (`stuck`), plan open | `--warning` | solid dot + warning-tinted border, tooltip carries `needLabel` |
| check | finished while you were away (`unread` / `fresh`) | `--green` | solid dot, clears on view |
| broken | dangling (reply never arrived) | `--danger` | solid dot |

Precedence: needs you > broken > running > check > idle. The tooltip and the history-modal row
both show the same reason text the sidebar shows today.

### History modal (`features/history/HistoryModal.tsx`, replaces `Sidebar.tsx`)

- `Modal` (wide). Search input at the top, autofocused; ↑/↓ + Enter open, Esc closes.
- Filters: current project by default, "All projects" toggle. Matches title, model, project
  name, and session id (client-side, instant).
- Rows reuse the sidebar row anatomy (title, need reason, date · model · dev-server port,
  status dot) and the sidebar's `sectionRows` ordering: Needs you first, then recent, settled
  last and quieted. Hover trash → existing delete confirm.
- Click → `resume(id)` (which registers the tab) and close the modal.

### Store changes (`lib/store.ts`)

- `chatTabs: Record<projectPath, { open: string[]; active: string | null }>`, persisted via a
  new `uiState` key. Restored at boot and filtered against `sessions` so deleted/never-started
  ids drop silently.
- `openTab(id)` (idempotent, called inside `resume` and `startNewSession` so every entry point
  gets a tab), `closeTab(id)` (picks the neighbour as active; closing the last tab starts a
  fresh chat, mirroring delete-the-current-chat), `closeOtherTabs`, `closeTabsRight`.
  `removeSessions` drops tabs. Closing never stops the agent: background chats keep running
  exactly as they do now, and a closed chat that starts needing you shows up in the history
  badge.
- `leftTab` stays (used by `revealInFiles`) but only "files" exists on the left now.

### Keyboard

⌘T new chat, ⌘W close tab, ⌘K (or ⌘⇧O) history modal, ⌃Tab / ⌃⇧Tab cycle, ⌘1…⌘9 jump.
Verify ⌘W isn't already bound by the Tauri window menu before relying on it.

## Phases

1. **Store + status** — `tabStatus.ts`, tab slice + persistence, `openTab` in `resume`/`startNewSession`, tests (`tabStatus.test.ts`, `store.test.ts`). Session-tag the question event.
2. **Tab strip** — `ChatTabs.tsx`, `tabs.css`, context menu, shortcuts, mounted in `Chat.tsx`; `ChatTabs.test.tsx`.
3. **History modal** — `HistoryModal.tsx` + search/keyboard, wired to the strip's history button; `HistoryModal.test.tsx` (ports the still-relevant Sidebar cases).
4. **Left column** — remove `history` dock, Files always-available + empty state + `DockToggle` in header, Settings gear in the title bar; fix `DockColumn.test.tsx`.
5. **Cleanup** — delete `Sidebar.tsx`/`Sidebar.test.tsx`, move shared CSS, update `DOCUMENT-MAP.md`, `app/README.md`, and the dock-system/background-chats notes. Run `npx tsc --noEmit && npx vitest run`.

## Open questions

1. Tab scope: per project (recommended, matches today's sidebar scoping and the Files dock) or one global strip across projects?
2. Closing a running tab: keep it running in the background and badge the history button when it needs you (recommended), auto-reopen its tab when it needs you, or refuse to close while running?
3. Colors: running = accent, needs intervention = warning, finished-unread = green, dangling/error = danger. Today unread uses accent, so green is the one new mapping. OK?
4. Settings and Home: gear in the title bar for Settings, keep the Home back-link atop the Files column. OK, or should Settings go somewhere else?
5. Search: client-side over titles/model/project now (recommended), with full-text over messages as a later backend endpoint. Or do you want full-text in this pass?
6. Strip placement: inside the chat column (recommended, docks unaffected) vs. a full-width strip under the title bar spanning the docks too.
7. Shortcuts: ⌘T / ⌘W / ⌘K / ⌃Tab / ⌘1–9 as listed. Any you want different?
