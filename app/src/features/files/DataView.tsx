// The data grid: CSV/TSV/JSONL/Parquet files as an Airtable-style table.
// Rows are windowed — the backend pages, sorts, and searches the file on
// disk, and the webview only ever holds the visible pages — so a
// million-row Parquet file scrolls like a small one. TanStack Table owns
// the column model (widths, resize drags); TanStack Virtual decides which
// absolute row indices to materialize; cells render straight from the
// sparse page cache. Click a header to sort, type to search, double-click
// (or press Enter on) a cell to edit it in place — edits write back to the
// file immediately, touching only the edited record.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import {
  ArrowDown,
  ArrowUp,
  Braces,
  Calendar,
  CalendarClock,
  Clock,
  Hash,
  List,
  Loader2,
  LockKeyhole,
  Search,
  Table2,
  ToggleLeft,
  Type,
  X,
} from "lucide-react";
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
  type ColumnSizingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { datasetQuery, datasetWriteCell } from "../../lib/ipc";
import { basename, formatBytes } from "../../lib/format";
import type { DatasetColumn, DatasetKind } from "../../lib/types";
import { useFsChanged } from "./useFsChanged";
import {
  PAGE_SIZE,
  editText,
  formatCell,
  gutterWidth,
  initialWidth,
  pageOf,
  parseEdit,
  type CellValue,
} from "./datafile";

const ROW_H = 28;
const HEADER_H = 30;
/** Page-cache ceiling (~12k rows) — far pages re-fetch rather than pile up. */
const MAX_CACHED_PAGES = 60;

const KIND_ICONS: Partial<Record<DatasetKind, typeof Hash>> = {
  int: Hash,
  float: Hash,
  bool: ToggleLeft,
  str: Type,
  date: Calendar,
  datetime: CalendarClock,
  time: Clock,
  duration: Clock,
  list: List,
  struct: Braces,
};

interface PageData {
  rows: CellValue[][];
  rowIds: number[];
}

interface GridMeta {
  columns: DatasetColumn[];
  totalRows: number;
  fileSize: number;
  format: string;
  elapsedMs: number;
  editable: boolean;
}

type Sort = { column: string; descending: boolean } | null;
type CellAddr = { row: number; col: number };

/** One materialized view row, or undefined while its page is in flight. */
function rowAt(pages: Map<number, PageData>, index: number): { cells: CellValue[]; id: number } | undefined {
  const page = pages.get(pageOf(index));
  if (!page) return undefined;
  const local = index - pageOf(index) * PAGE_SIZE;
  const cells = page.rows[local];
  return cells === undefined ? undefined : { cells, id: page.rowIds[local] };
}

export function DataView({
  workspace,
  path,
  onClose,
  actions,
}: {
  workspace: string;
  path: string;
  onClose: () => void;
  /** Extra header controls (the Table/Raw toggle), injected by the pane. */
  actions?: ReactNode;
}) {
  const [meta, setMeta] = useState<GridMeta | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sort, setSort] = useState<Sort>(null);
  const [searchText, setSearchText] = useState("");
  const [search, setSearch] = useState("");
  const [version, setVersion] = useState(0);
  const [, setPageStamp] = useState(0);
  const [selected, setSelected] = useState<CellAddr | null>(null);
  const [editing, setEditing] = useState<(CellAddr & { text: string }) | null>(null);
  const [editError, setEditError] = useState<string | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  const pagesRef = useRef(new Map<number, PageData>());
  const inflightRef = useRef(new Set<number>());
  const genRef = useRef(0);
  // Rows in the unfiltered file, remembered so a filtered footer can say
  // "12 of 5,000,000 rows".
  const unfilteredRef = useRef<number | null>(null);
  // Our own cell writes echo back as watcher events; skip the reload we'd
  // otherwise do (the page cache is already patched).
  const selfEditAt = useRef(0);
  // The file mtime our pages were read at — sent with every write so an edit
  // against a file that changed underneath is refused by the backend.
  const mtimeRef = useRef<number | null>(null);

  // ---- fetching -----------------------------------------------------------

  const loadPage = useCallback(
    async (page: number) => {
      if (pagesRef.current.has(page) || inflightRef.current.has(page)) return;
      const gen = genRef.current;
      inflightRef.current.add(page);
      try {
        const result = await datasetQuery(workspace, path, {
          offset: page * PAGE_SIZE,
          limit: PAGE_SIZE,
          sortBy: sort?.column,
          descending: sort?.descending,
          search: search || undefined,
        });
        if (gen !== genRef.current) return; // view changed while in flight
        pagesRef.current.set(page, { rows: result.rows, rowIds: result.rowIds });
        // Keep only the pages nearest the one just fetched — a scrollbar drag
        // through a huge file must not accumulate every window it passed.
        if (pagesRef.current.size > MAX_CACHED_PAGES) {
          const keys = [...pagesRef.current.keys()].sort(
            (a, b) => Math.abs(b - page) - Math.abs(a - page),
          );
          for (const k of keys.slice(0, pagesRef.current.size - MAX_CACHED_PAGES)) {
            pagesRef.current.delete(k);
          }
        }
        mtimeRef.current = result.mtimeMs;
        setMeta({
          columns: result.columns,
          totalRows: result.totalRows,
          fileSize: result.fileSize,
          format: result.format,
          elapsedMs: result.elapsedMs,
          editable: result.editable,
        });
        if (!search) unfilteredRef.current = result.totalRows;
        setError(null);
        setPageStamp((s) => s + 1);
      } catch (e) {
        if (gen === genRef.current) setError(String(e));
      } finally {
        inflightRef.current.delete(page);
      }
    },
    [workspace, path, sort, search],
  );

  // A new view (sort/search/file version) drops every cached page. Sort and
  // search also jump back to the top; an on-disk refresh keeps the scroll.
  const viewKey = `${sort?.column ?? ""}:${sort?.descending ?? false}:${search}`;
  useEffect(() => {
    genRef.current++;
    pagesRef.current.clear();
    inflightRef.current.clear();
    setEditing(null);
    void loadPage(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, path, viewKey, version]);
  useEffect(() => {
    scrollRef.current?.scrollTo?.({ top: 0 });
    setSelected(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewKey]);

  useEffect(() => {
    const t = window.setTimeout(() => setSearch(searchText.trim()), 300);
    return () => window.clearTimeout(t);
  }, [searchText]);

  useFsChanged(workspace, [path], () => {
    if (Date.now() - selfEditAt.current < 2000) return;
    setVersion((v) => v + 1);
  });

  // ---- the column model (TanStack Table: widths + resize drags) ------------

  const columns = useMemo(() => meta?.columns ?? [], [meta?.columns]);
  const firstPage = pagesRef.current.get(0);
  const columnDefs = useMemo<ColumnDef<number>[]>(
    () =>
      columns.map((c, ci) => ({
        id: c.name,
        size: initialWidth(
          c.name,
          c.kind,
          (firstPage?.rows ?? []).slice(0, 40).map((r) => formatCell(r[ci], c.kind)),
        ),
        minSize: 64,
        maxSize: 800,
        header: c.name,
      })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [columns, firstPage === undefined],
  );
  const [columnSizing, setColumnSizing] = useState<ColumnSizingState>({});
  const table = useReactTable({
    data: EMPTY_DATA, // rows render from the page cache, not a client row model
    columns: columnDefs,
    state: { columnSizing },
    onColumnSizingChange: setColumnSizing,
    columnResizeMode: "onChange",
    getCoreRowModel: getCoreRowModel(),
  });
  const headers = table.getFlatHeaders();
  const widths = headers.map((h) => h.getSize());
  const gutter = gutterWidth(meta?.totalRows ?? 0);
  const gridWidth = gutter + widths.reduce((a, b) => a + b, 0);

  // ---- virtualized rows -----------------------------------------------------

  const totalRows = meta?.totalRows ?? 0;
  const virtualizer = useVirtualizer({
    count: totalRows,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 12,
    // The sticky header lives inside the scroller; padding the rows down by
    // its height keeps scrollToIndex from tucking rows underneath it.
    paddingStart: HEADER_H,
  });
  const items = virtualizer.getVirtualItems();
  const firstIndex = items[0]?.index ?? 0;
  const lastIndex = items[items.length - 1]?.index ?? 0;
  useEffect(() => {
    if (!meta) return;
    for (let p = pageOf(firstIndex); p <= pageOf(lastIndex); p++) void loadPage(p);
  }, [meta, firstIndex, lastIndex, loadPage]);

  // ---- editing ---------------------------------------------------------------

  // Nested values never edit; parquet temporals don't either (the backend's
  // string→date cast would fail — better to not offer than to error).
  const canEditColumn = (col: DatasetColumn) => {
    if (!meta?.editable) return false;
    if (col.kind === "list" || col.kind === "struct") return false;
    if (meta.format === "parquet" && ["date", "datetime", "time", "duration"].includes(col.kind))
      return false;
    return true;
  };

  /** Editing steals focus into the cell input; hand it back so arrow-key
   *  navigation keeps working after a commit or cancel. */
  function focusGrid() {
    scrollRef.current?.focus({ preventScroll: true });
  }

  function startEdit(addr: CellAddr) {
    const col = columns[addr.col];
    const row = rowAt(pagesRef.current, addr.row);
    if (!col || !row || !canEditColumn(col)) return;
    setEditError(null);
    setEditing({ ...addr, text: editText(row.cells[addr.col]) });
  }

  async function commitEdit(move: "down" | "right" | null) {
    if (!editing) return;
    const col = columns[editing.col];
    const row = rowAt(pagesRef.current, editing.row);
    if (!col || !row) return setEditing(null);
    const parsed = parseEdit(editing.text, col.kind);
    if (parsed === undefined) {
      setEditError(`not a valid ${col.kind}`);
      return;
    }
    setEditing(null);
    setEditError(null);
    focusGrid();
    if (move) moveSelection(move === "down" ? 1 : 0, move === "right" ? 1 : 0, editing);
    if (parsed === row.cells[editing.col]) return;
    // Optimistic: the grid shows the new value while the write lands.
    const page = pagesRef.current.get(pageOf(editing.row));
    if (page) {
      page.rows[editing.row - pageOf(editing.row) * PAGE_SIZE][editing.col] = parsed;
      setPageStamp((s) => s + 1);
    }
    selfEditAt.current = Date.now();
    try {
      mtimeRef.current = await datasetWriteCell(
        workspace,
        path,
        row.id,
        col.name,
        parsed,
        mtimeRef.current ?? undefined,
      );
    } catch (e) {
      setError(String(e));
      setVersion((v) => v + 1); // reload the truth from disk
    }
  }

  // ---- selection + keyboard -----------------------------------------------

  function moveSelection(dr: number, dc: number, from?: CellAddr) {
    const base = from ?? selected;
    if (!base || !columns.length || !totalRows) return;
    const row = Math.min(Math.max(base.row + dr, 0), totalRows - 1);
    const col = Math.min(Math.max(base.col + dc, 0), columns.length - 1);
    setSelected({ row, col });
    virtualizer.scrollToIndex(row);
  }

  function onGridKeyDown(e: KeyboardEvent) {
    if (editing) return; // the input owns the keyboard
    const nav: Record<string, [number, number]> = {
      ArrowUp: [-1, 0],
      ArrowDown: [1, 0],
      ArrowLeft: [0, -1],
      ArrowRight: [0, 1],
      PageUp: [-20, 0],
      PageDown: [20, 0],
    };
    if (e.key in nav) {
      e.preventDefault();
      if (selected) moveSelection(...nav[e.key]);
      else if (totalRows) setSelected({ row: firstIndex, col: 0 });
    } else if (e.key === "Enter" && selected) {
      e.preventDefault();
      startEdit(selected);
    } else if (e.key === "Escape") {
      setSelected(null);
    } else if (e.key === "Tab" && selected) {
      e.preventDefault();
      moveSelection(0, e.shiftKey ? -1 : 1);
    }
  }

  function cycleSort(column: string) {
    setSort((s) =>
      s?.column !== column
        ? { column, descending: false }
        : s.descending
          ? null
          : { column, descending: true },
    );
  }

  // ---- render ---------------------------------------------------------------

  const loading = !meta && !error;
  const filtered = !!search && unfilteredRef.current !== null;
  return (
    <>
      <header className="canvas-head editor-head">
        <div className="editor-path" title={path}>
          <Table2 size={14} aria-hidden="true" />
          <span className="editor-fname">{basename(path)}</span>
          {meta && !meta.editable && (
            <span className="editor-note">
              <LockKeyhole size={10} aria-hidden="true" /> read-only
            </span>
          )}
        </div>
        <div className="editor-actions">
          <div className="dataview-search">
            <Search size={12} aria-hidden="true" />
            <input
              type="search"
              value={searchText}
              placeholder="Search rows"
              aria-label="Search rows"
              spellCheck={false}
              onChange={(e) => setSearchText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") setSearchText("");
              }}
            />
            {searchText && (
              <button aria-label="Clear search" onClick={() => setSearchText("")}>
                <X size={11} />
              </button>
            )}
          </div>
          {actions}
          <button className="icon-btn sm" aria-label="Close editor" title="Close" onClick={onClose}>
            <X size={15} />
          </button>
        </div>
      </header>
      {error && <p className="editor-error">{error}</p>}
      <div
        className="dataview-grid"
        ref={scrollRef}
        tabIndex={0}
        role="grid"
        aria-label={`${basename(path)} data`}
        aria-rowcount={totalRows}
        aria-colcount={columns.length}
        onKeyDown={onGridKeyDown}
      >
        {loading && (
          <div className="dataview-status">
            <Loader2 size={16} className="spin" aria-hidden="true" />
            Reading {basename(path)}…
          </div>
        )}
        {meta && totalRows === 0 && (
          <div className="dataview-status">{search ? `No rows match “${search}”` : "No rows"}</div>
        )}
        {meta && columns.length > 0 && (
          <div className="dataview-inner" style={{ width: gridWidth, height: virtualizer.getTotalSize() }}>
            <div className="dataview-hrow" style={{ width: gridWidth, height: HEADER_H }} role="row">
              <div className="dataview-hcell dataview-gutter" style={{ width: gutter }} aria-hidden="true" />
              {headers.map((header, ci) => {
                const col = columns[ci];
                const Icon = KIND_ICONS[col.kind] ?? Type;
                const active = sort?.column === col.name;
                return (
                  <div
                    key={header.id}
                    className={`dataview-hcell${active ? " sorted" : ""}`}
                    style={{ width: widths[ci] }}
                    role="columnheader"
                    aria-sort={active ? (sort!.descending ? "descending" : "ascending") : "none"}
                    title={`${col.name} — ${col.dtype}. Click to sort.`}
                    onClick={() => cycleSort(col.name)}
                  >
                    <Icon size={11} className="dataview-hicon" aria-hidden="true" />
                    <span className="dataview-htitle">
                      {flexRender(header.column.columnDef.header, header.getContext())}
                    </span>
                    {active &&
                      (sort!.descending ? (
                        <ArrowDown size={11} aria-hidden="true" />
                      ) : (
                        <ArrowUp size={11} aria-hidden="true" />
                      ))}
                    <div
                      className="dataview-resize"
                      onClick={(e) => e.stopPropagation()}
                      onDoubleClick={() => header.column.resetSize()}
                      onMouseDown={header.getResizeHandler()}
                      onTouchStart={header.getResizeHandler()}
                    />
                  </div>
                );
              })}
            </div>
            {items.map((vi) => {
              const row = rowAt(pagesRef.current, vi.index);
              return (
                <div
                  key={vi.key}
                  className="dataview-row"
                  style={{ transform: `translateY(${vi.start}px)`, width: gridWidth, height: ROW_H }}
                  role="row"
                  aria-rowindex={vi.index + 1}
                >
                  <div className="dataview-cell dataview-gutter num" style={{ width: gutter }}>
                    {vi.index + 1}
                  </div>
                  {columns.map((col, ci) => {
                    if (!row) {
                      return (
                        <div key={col.name} className="dataview-cell" style={{ width: widths[ci] }}>
                          <span className="dataview-skeleton" />
                        </div>
                      );
                    }
                    const value = row.cells[ci];
                    const isSelected = selected?.row === vi.index && selected.col === ci;
                    const isEditing = editing?.row === vi.index && editing.col === ci;
                    const numeric = col.kind === "int" || col.kind === "float";
                    return (
                      <div
                        key={col.name}
                        className={`dataview-cell${numeric ? " num" : ""}${col.kind === "bool" ? " bool" : ""}${isSelected ? " selected" : ""}`}
                        style={{ width: widths[ci] }}
                        role="gridcell"
                        title={value === null ? undefined : String(value)}
                        onClick={() => setSelected({ row: vi.index, col: ci })}
                        onDoubleClick={() => startEdit({ row: vi.index, col: ci })}
                      >
                        {isEditing ? (
                          <input
                            className={`dataview-editor${editError ? " invalid" : ""}`}
                            autoFocus
                            value={editing.text}
                            title={editError ?? undefined}
                            aria-invalid={!!editError}
                            aria-label={`Edit ${col.name}, row ${vi.index + 1}`}
                            spellCheck={false}
                            onChange={(e) => {
                              setEditError(null);
                              setEditing({ ...editing, text: e.target.value });
                            }}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") void commitEdit("down");
                              else if (e.key === "Tab") {
                                e.preventDefault();
                                void commitEdit("right");
                              } else if (e.key === "Escape") {
                                setEditing(null);
                                focusGrid();
                              }
                              e.stopPropagation();
                            }}
                            onBlur={() => void commitEdit(null)}
                          />
                        ) : value === null ? (
                          <span className="dataview-null">∅</span>
                        ) : (
                          formatCell(value, col.kind)
                        )}
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>
        )}
      </div>
      {meta && (
        <footer className="dataview-foot">
          <span className="dataview-pill">{meta.format}</span>
          <span>
            {filtered
              ? `${meta.totalRows.toLocaleString()} of ${unfilteredRef.current!.toLocaleString()} rows`
              : `${meta.totalRows.toLocaleString()} rows`}
            {" × "}
            {columns.length} cols
          </span>
          <span>{formatBytes(meta.fileSize)}</span>
          <span className="dataview-foot-right">
            {selected && `r${selected.row + 1} · ${columns[selected.col]?.name ?? ""} — `}
            {meta.elapsedMs} ms
          </span>
        </footer>
      )}
    </>
  );
}

/** TanStack Table only manages the column model here (sizing/resizing);
 *  rows render straight from the sparse page cache. */
const EMPTY_DATA: number[] = [];
