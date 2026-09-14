import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { MediaPanel } from "./MediaPanel";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaUpload } from "../../lib/types";

const upload = (over: Partial<MediaUpload> = {}): MediaUpload => ({
  id: "u1",
  session: "s1",
  label: "[Image #1]",
  filename: "photo.png",
  kind: "image",
  bytes_sent: 640 * 1024,
  bytes_total: 1000 * 1024,
  status: "uploading",
  error: null,
  started_at: 1_789_000_000,
  ...over,
});

beforeEach(() => {
  resetAll();
  useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" } });
});

describe("MediaPanel uploads", () => {
  it("shows an upload row with its percent and hides finished ones", () => {
    useStore.setState({
      mediaUploads: {
        "/w": [upload(), upload({ id: "u2", filename: "done.png", status: "done", bytes_sent: 1, bytes_total: 1 })],
      },
    });
    render(<MediaPanel />);
    expect(screen.getByText("Uploading 1 reference…")).toBeInTheDocument();
    expect(screen.getByText("photo.png")).toBeInTheDocument();
    expect(screen.getByText("[Image #1]")).toBeInTheDocument();
    expect(screen.getByText("64% · 640 KB of 1000 KB")).toBeInTheDocument();
    expect(screen.queryByText("done.png")).not.toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Uploading photo.png" })).toHaveAttribute("aria-valuenow", "64");
  });

  it("keeps a failed upload visible with its error and stays hidden with nothing in flight", () => {
    useStore.setState({ mediaUploads: { "/w": [upload({ status: "failed", error: "hub said no" })] } });
    render(<MediaPanel />);
    expect(screen.getByText("A reference upload failed")).toBeInTheDocument();
    expect(screen.getByText("hub said no")).toBeInTheDocument();
  });

  it("renders nothing when there are no uploads or generations", () => {
    const { container } = render(<MediaPanel />);
    expect(container).toBeEmptyDOMElement();
  });
});
