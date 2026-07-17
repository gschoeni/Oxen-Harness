import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { DataView } from "./DataView";
import { datasetQuery, datasetWriteCell, sampleSession } from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

const ROOT = sampleSession.workspace;

const samplePage = {
  columns: [
    { name: "id", dtype: "i64", kind: "int" },
    { name: "name", dtype: "str", kind: "str" },
    { name: "score", dtype: "f64", kind: "float" },
  ],
  rows: [
    [1, "ada", 0.5],
    [2, "grace", 0.9],
    [3, null, 0.7],
  ],
  rowIds: [0, 1, 2],
  totalRows: 3,
  fileSize: 1234,
  format: "csv",
  elapsedMs: 2,
  editable: true,
  mtimeMs: 111,
};

beforeEach(() => {
  resetAll();
  datasetQuery.mockImplementation(async () => structuredClone(samplePage));
  // The virtualizer sizes its viewport from offsetWidth/offsetHeight, which
  // jsdom always reports as 0 — give every element a real-looking box so
  // rows materialize.
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(800);
});

function renderView() {
  return render(<DataView workspace={ROOT} path="data.csv" onClose={() => {}} />);
}

describe("DataView", () => {
  it("renders the window it fetched: headers, cells, nulls, and the footer", async () => {
    renderView();
    expect(await screen.findByText("grace")).toBeInTheDocument();
    expect(datasetQuery).toHaveBeenCalledWith(ROOT, "data.csv", {
      offset: 0,
      limit: 200,
      sortBy: undefined,
      descending: undefined,
      search: undefined,
    });
    expect(screen.getByRole("columnheader", { name: /name/ })).toBeInTheDocument();
    expect(screen.getByText("∅")).toBeInTheDocument(); // null cell
    expect(screen.getByText("3 rows × 3 cols")).toBeInTheDocument();
    expect(screen.getByText("csv")).toBeInTheDocument();
  });

  it("cycles sort on header click and refetches server-side", async () => {
    renderView();
    await screen.findByText("ada");
    await userEvent.click(screen.getByRole("columnheader", { name: /score/ }));
    await waitFor(() =>
      expect(datasetQuery).toHaveBeenCalledWith(
        ROOT,
        "data.csv",
        expect.objectContaining({ sortBy: "score", descending: false }),
      ),
    );
    await userEvent.click(screen.getByRole("columnheader", { name: /score/ }));
    await waitFor(() =>
      expect(datasetQuery).toHaveBeenCalledWith(
        ROOT,
        "data.csv",
        expect.objectContaining({ sortBy: "score", descending: true }),
      ),
    );
  });

  it("searches server-side after the debounce", async () => {
    renderView();
    await screen.findByText("ada");
    await userEvent.type(screen.getByRole("searchbox", { name: "Search rows" }), "gra");
    await waitFor(() =>
      expect(datasetQuery).toHaveBeenCalledWith(
        ROOT,
        "data.csv",
        expect.objectContaining({ search: "gra" }),
      ),
    );
  });

  it("edits a cell in place and writes it back by physical row", async () => {
    renderView();
    const cell = await screen.findByText("grace");
    await userEvent.dblClick(cell);
    const input = screen.getByRole("textbox", { name: /Edit name/ });
    await userEvent.clear(input);
    await userEvent.type(input, "hopper{Enter}");
    await waitFor(() =>
      expect(datasetWriteCell).toHaveBeenCalledWith(ROOT, "data.csv", 1, "name", "hopper", 111),
    );
    expect(screen.getByText("hopper")).toBeInTheDocument(); // optimistic
  });

  it("rejects an edit that doesn't fit the column dtype", async () => {
    renderView();
    const cell = await screen.findByText("0.9");
    await userEvent.dblClick(cell);
    const input = screen.getByRole("textbox", { name: /Edit score/ });
    await userEvent.clear(input);
    await userEvent.type(input, "not-a-number{Enter}");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(datasetWriteCell).not.toHaveBeenCalled();
  });

  it("marks huge parquet files read-only instead of offering edits", async () => {
    datasetQuery.mockImplementation(async () => ({ ...structuredClone(samplePage), format: "parquet", editable: false }));
    renderView();
    const cell = await screen.findByText("grace");
    expect(screen.getByText(/read-only/)).toBeInTheDocument();
    await userEvent.dblClick(cell);
    expect(screen.queryByRole("textbox", { name: /Edit/ })).not.toBeInTheDocument();
  });
});
