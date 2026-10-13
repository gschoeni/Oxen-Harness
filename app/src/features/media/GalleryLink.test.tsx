import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { GalleryLink } from "./GalleryLink";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";
import type { MediaItem } from "../../lib/types";

const item = (over: Partial<MediaItem>): MediaItem =>
  ({ id: "g1", session: "s1", kind: "image", status: "succeeded", path: "generations/a.png", ...over }) as MediaItem;

beforeEach(() => {
  resetAll();
  useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" } });
});

describe("GalleryLink", () => {
  it("stays out of a project that has generated nothing", async () => {
    const { container } = render(<GalleryLink workspace="/w" />);
    expect(ipc.listMedia).toHaveBeenCalledWith("/w");
    expect(container).toBeEmptyDOMElement();
  });

  it("opens the gallery grid in the work view and marks itself current", async () => {
    useStore.setState({
      media: {
        "/w": [
          item({}),
          item({ id: "g2" }),
          item({ id: "v1", kind: "video", status: "processing", path: null }),
          item({ id: "x", status: "cancelled" }),
        ],
      },
    });
    render(<GalleryLink workspace="/w" />);
    const link = screen.getByRole("button", { name: /Gallery/ });
    expect(link).toHaveTextContent("3");
    expect(screen.getByRole("button", { name: "1 rendering" })).toBeInTheDocument();
    expect(link).not.toHaveAttribute("aria-current");

    await userEvent.click(link);
    expect(useStore.getState().workContexts.s1.current).toEqual({ view: "gallery" });
    expect(useStore.getState().rightTab.s1).toBe("gallery");
    expect(link).toHaveAttribute("aria-current", "page");
  });

  it("opens the gallery on what is still rendering from its pill", async () => {
    useStore.setState({
      media: { "/w": [item({}), item({ id: "q", status: "queued", path: null })] },
    });
    render(<GalleryLink workspace="/w" />);
    await userEvent.click(screen.getByRole("button", { name: "1 rendering" }));
    expect(useStore.getState().workContexts.s1.current).toEqual({ view: "gallery" });
    expect(useStore.getState().galleryRenderingFocus).toBe(true);
  });

  it("shows no pill when nothing is rendering", () => {
    useStore.setState({ media: { "/w": [item({})] } });
    render(<GalleryLink workspace="/w" />);
    expect(screen.queryByRole("button", { name: /rendering/ })).not.toBeInTheDocument();
  });
});
