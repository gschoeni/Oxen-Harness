import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { AttachmentImage } from "./AttachmentImage";
import { useStore } from "../../lib/store";
import * as ipc from "../../test/ipcMock";
import { resetAll } from "../../test/utils";

beforeEach(() => {
  resetAll();
  useStore.setState({ session: { ...ipc.sampleSession, session_id: "s1", workspace: "/w" } });
});

describe("AttachmentImage", () => {
  it("becomes a drag source carrying the resolved absolute path", async () => {
    const { container } = render(<AttachmentImage src="attachments/photo.png" alt="photo" />);
    const img = await waitFor(() => {
      const el = container.querySelector("img");
      expect(el).toBeTruthy();
      return el as HTMLImageElement;
    });
    expect(img.getAttribute("draggable")).toBe("true");
    const setData = vi.fn();
    fireEvent.dragStart(img, { dataTransfer: { setData, types: [], effectAllowed: "none" } });
    expect(setData).toHaveBeenCalledWith("application/x-oxen-workspace-files", JSON.stringify(["/ws/attachments/photo.png"]));
  });

  it("renders a video as a draggable thumbnail that becomes a player on click", async () => {
    const { container } = render(<AttachmentImage src="/tmp/clip.mp4" />);
    const button = await screen.findByRole("button", { name: "Play video attachment" });
    expect(button.getAttribute("draggable")).toBe("true");
    expect(container.querySelector(".video-still-play")).not.toBeNull();
    expect(container.querySelector("video[controls]")).toBeNull();

    fireEvent.click(button);
    const video = container.querySelector("video") as HTMLVideoElement;
    expect(video.hasAttribute("controls")).toBe(true);
    expect(video.getAttribute("draggable")).toBe("true");
  });

  it("keeps a tray video a still thumbnail", async () => {
    const { container } = render(<AttachmentImage src="/tmp/clip.mp4" thumbnail />);
    await waitFor(() => expect(container.querySelector(".video-still video")).toBeTruthy());
    expect(screen.queryByRole("button")).toBeNull();
    expect(container.querySelector("video[controls]")).toBeNull();
  });
});
