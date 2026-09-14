import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, waitFor } from "@testing-library/react";

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

  it("renders a draggable player for a video attachment", async () => {
    const { container } = render(<AttachmentImage src="/tmp/clip.mp4" />);
    const video = await waitFor(() => {
      const el = container.querySelector("video");
      expect(el).toBeTruthy();
      return el as HTMLVideoElement;
    });
    expect(video.getAttribute("draggable")).toBe("true");
  });
});
