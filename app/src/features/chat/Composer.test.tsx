import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../../lib/ipc", () => import("../../test/ipcMock"));
vi.mock("./ModelPicker", async (importOriginal) => {
  const original = await importOriginal<typeof import("./ModelPicker")>();
  return { ModelPicker: vi.fn(original.ModelPicker) };
});

import { resetAll } from "../../test/utils";
import { Composer, attachmentCountLabel } from "./Composer";
import { ModelPicker } from "./ModelPicker";
import { useStore } from "../../lib/store";
import { sampleSession } from "../../test/ipcMock";

beforeEach(resetAll);
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

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

function mockAnimationFrames() {
  const frames = new Map<number, FrameRequestCallback>();
  let next = 0;
  vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
    frames.set(++next, callback);
    return next;
  });
  vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => { frames.delete(id); });
  const flush = () => act(() => {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(0));
  });
  return { frames, flush };
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

describe("composer typing", () => {
  it("updates text immediately without measuring layout or re-rendering the toolbar", () => {
    mount([]);
    const box = screen.getByRole("textbox");
    const measure = vi.spyOn(box, "scrollHeight", "get").mockReturnValue(36);
    vi.mocked(ModelPicker).mockClear();
    fireEvent.change(box, { target: { value: "a quick draft" } });
    expect(box).toHaveValue("a quick draft");
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
    expect(measure).not.toHaveBeenCalled();
    expect(ModelPicker).not.toHaveBeenCalled();
  });

  it("coalesces fallback resizing and shrinks again after sending", () => {
    vi.stubGlobal("CSS", { supports: () => false });
    const { frames, flush } = mockAnimationFrames();
    const onSend = vi.fn();
    const { unmount } = render(
      <Composer busy={false} onSend={onSend} onStop={() => {}} onAttach={() => {}} />,
    );
    const box = screen.getByRole("textbox");
    const measure = vi.spyOn(box, "scrollHeight", "get").mockReturnValue(300);
    fireEvent.change(box, { target: { value: "first line\nsecond line" } });
    fireEvent.change(box, { target: { value: "first line\nsecond line\nthird line" } });
    expect(measure).not.toHaveBeenCalled();
    expect(frames.size).toBe(1);
    flush();
    expect(measure).toHaveBeenCalledTimes(1);
    expect(box.style.height).toBe("200px");
    fireEvent.keyDown(box, { key: "Enter" });
    expect(onSend).toHaveBeenCalledWith("first line\nsecond line\nthird line");
    expect(box).toHaveValue("");
    measure.mockReturnValue(36);
    flush();
    expect(box.style.height).toBe("36px");
    fireEvent.change(box, { target: { value: "pending" } });
    unmount();
    expect(frames.size).toBe(0);
  });

  it("leaves sizing to the browser when field-sizing is available", () => {
    vi.stubGlobal("CSS", { supports: () => true });
    const raf = vi.spyOn(window, "requestAnimationFrame");
    mount([]);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "native sizing" } });
    expect(raf).not.toHaveBeenCalled();
  });

  it("keeps the toolbar live when the session or run state changes", () => {
    const props = { onSend: vi.fn(), onStop: vi.fn(), onAttach: vi.fn() };
    const { rerender } = render(<Composer {...props} busy={false} focusKey="first" />);
    act(() => useStore.setState({ session: { ...sampleSession, model: "new-model" } }));
    expect(screen.getByText("new-model")).toBeInTheDocument();
    rerender(<Composer {...props} busy={true} focusKey="second" />);
    expect(screen.getByRole("textbox")).toHaveFocus();
    expect(screen.getByTitle("Finish the current turn to switch models")).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "queued draft" } });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    expect(props.onSend).toHaveBeenCalledWith("queued draft");
  });

  it("resizes the fallback on width changes without looping on height changes", () => {
    vi.stubGlobal("CSS", { supports: () => false });
    let observerCallback: ResizeObserverCallback = () => {};
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: ResizeObserverCallback) { observerCallback = callback; }
      observe() {}
      disconnect = disconnect;
    });
    const { frames, flush } = mockAnimationFrames();
    const { unmount } = render(
      <Composer busy={false} onSend={() => {}} onStop={() => {}} onAttach={() => {}} />,
    );
    const box = screen.getByRole("textbox");
    const measure = vi.spyOn(box, "scrollHeight", "get").mockReturnValue(40);
    flush();
    const resize = (width: number) => act(() => observerCallback(
      [{ contentRect: { width } } as ResizeObserverEntry], {} as ResizeObserver,
    ));
    resize(500);
    flush();
    resize(500);
    expect(frames.size).toBe(0);
    measure.mockReturnValue(100);
    resize(250);
    flush();
    expect(box.style.height).toBe("100px");
    unmount();
    expect(disconnect).toHaveBeenCalledOnce();
  });
});
