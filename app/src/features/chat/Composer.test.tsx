import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));

import { resetAll } from "../../test/utils";
import { Composer, attachmentCountLabel } from "./Composer";

beforeEach(resetAll);

const three = [
  { path: "/w/generations/a.png", name: "a.png" },
  { path: "/w/generations/b.png", name: "b.png" },
  { path: "/w/clip.mp4", name: "clip.mp4" },
];

function mount(attachments = three) {
  const onRemove = vi.fn();
  const onClear = vi.fn();
  render(
    <Composer
      busy={false}
      onSend={() => {}}
      onStop={() => {}}
      onAttach={() => {}}
      attachments={attachments}
      onRemoveAttachment={onRemove}
      onClearAttachments={onClear}
    />,
  );
  return { onRemove, onClear };
}

describe("composer media tray", () => {
  it("counts what is staged, per kind", () => {
    expect(attachmentCountLabel(three)).toBe("2 images · 1 video");
    expect(attachmentCountLabel([three[0]])).toBe("1 image");
    expect(attachmentCountLabel([{ path: "/w/song.mp3", name: "song.mp3" }])).toBe("1 audio track");
    expect(attachmentCountLabel([{ path: "/w/notes.pdf", name: "notes.pdf" }])).toBe("1 file");
  });

  it("shows one tile per staged file with the count label inside the bar", () => {
    mount();
    const tray = screen.getByRole("list", { name: /attached media/i });
    expect(tray.closest(".composer-inner")).not.toBeNull();
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
    expect(screen.getByText("2 images · 1 video")).toBeInTheDocument();
    expect(screen.getByTitle("clip.mp4")).toBeInTheDocument();
  });

  it("removes one tile with its ✕ and everything with Clear", async () => {
    const { onRemove, onClear } = mount();
    await userEvent.click(screen.getByRole("button", { name: /remove b\.png/i }));
    expect(onRemove).toHaveBeenCalledWith(1);
    await userEvent.click(screen.getByRole("button", { name: /^clear$/i }));
    expect(onClear).toHaveBeenCalledTimes(1);
  });

  it("offers Clear only for two or more files", () => {
    mount([three[0]]);
    expect(screen.queryByRole("button", { name: /^clear$/i })).toBeNull();
  });

  it("backspace on an empty prompt removes the last staged file, not a typed one", async () => {
    const { onRemove } = mount();
    const box = screen.getByPlaceholderText(/ask the agent/i);
    await userEvent.click(box);
    await userEvent.keyboard("{Backspace}");
    expect(onRemove).toHaveBeenCalledWith(2);
    await userEvent.type(box, "hi");
    await userEvent.keyboard("{Backspace}");
    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});
