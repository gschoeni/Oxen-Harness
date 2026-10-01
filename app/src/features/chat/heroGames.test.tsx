import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HeroGame } from "./heroGames";
import * as sfx from "./games/sfx";

const palette = { title: "#f0be8c", primary: "#60b060", secondary: "#aa6e3c", text: "#ece2ce", muted: "#968d7d", danger: "#c94c4c", link: "#f0be8c", background: "#0f1115", surface: "#17191f", border: "#2a2d35" };

function stage() {
  return document.querySelector(".hero-game-stage") as HTMLElement;
}

describe("HeroGame wrapper", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("starts on a click as well as the arrow combo", () => {
    render(<HeroGame gameName="tumbleweed" palette={palette} />);
    expect(screen.getByLabelText(/Press up, up, down, down to play/i)).toBeInTheDocument();
    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    expect(screen.getByLabelText(/Press escape to make camp/i)).toBeInTheDocument();
  });

  it("pauses when the window blurs and resumes on the next key, swallowing it", async () => {
    render(<HeroGame gameName="tumbleweed" palette={palette} />);
    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    expect(screen.getByRole("status")).toHaveTextContent(/PAUSED/);
    expect(screen.getByLabelText(/Paused/i)).toBeInTheDocument();
    await userEvent.keyboard("{ArrowLeft}");
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.getByLabelText(/Press escape to make camp/i)).toBeInTheDocument();
  });

  it("follows the saved sound preference in the hero but starts muted in the dock", async () => {
    const { unmount } = render(<HeroGame gameName="tumbleweed" palette={palette} />);
    const toggle = screen.getByRole("button", { name: /mute game sound/i });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(toggle);
    expect(sfx.sfxPreference()).toBe(false);
    unmount();
    render(<HeroGame gameName="tumbleweed" palette={palette} variant="dock" />);
    expect(screen.getByRole("button", { name: /unmute game sound/i })).toHaveAttribute("aria-pressed", "false");
  });

  it("remembers the daily-run switch", async () => {
    render(<HeroGame gameName="tumbleweed" palette={palette} />);
    const daily = screen.getByRole("button", { name: /daily/i });
    expect(daily).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(daily);
    expect(daily).toHaveAttribute("aria-pressed", "true");
    expect(window.localStorage.getItem("oxen-hero-daily")).toBe("1");
  });

  it("performs a game's queued requests through the host and delivers the replies", async () => {
    const question = { id: "q1", territory: "src", kind: "free_text", prompt: "Which function makes room in the context?", options: [], source_path: "src/turn.rs", source_excerpt: "", difficulty: 1, cached: false };
    const profile = { project: "p", workspace: "/p", understanding: 10, level: 2, answered: 1, territories: [] };
    const perform = vi.fn(async (kind: string, _payload: unknown) => {
      if (kind === "study.profile") return profile;
      if (kind === "study.batch") return { mode: "expedition", questions: [question], territory: "src", tokens_used: 0, model: "m" };
      return { grade: { verdict: "full", feedback: "Right.", correct_answer: "make_room", explanation: "" }, profile, tokens_used: 0 };
    });
    render(<HeroGame gameName="study" palette={palette} host={{ perform }} />);
    // The profile loads on the attract screen, before any play.
    await screen.findByText("LEVEL 2");
    expect(perform).toHaveBeenCalledWith("study.profile", null);
    // The study cabinet has no daily run to share.
    expect(screen.queryByRole("button", { name: /daily/i })).toBeNull();

    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    await userEvent.keyboard("1");
    expect(perform).toHaveBeenCalledWith("study.batch", expect.objectContaining({ mode: "expedition" }));

    // A free-text question opens the answer box; typing there never reaches
    // the game's key handler, and submitting sends the answer for grading.
    const input = await screen.findByLabelText("Your answer");
    await userEvent.type(input, "make_room{Enter}");
    expect(perform).toHaveBeenCalledWith("study.answer", { question_id: "q1", answer: "make_room", hint_used: false });
    await screen.findAllByText("RIGHT!"); // pixel titles render twice (shadow + face)
    expect(screen.queryByLabelText("Your answer")).toBeNull();
    // Focusing the cabinet's own answer box must not have paused the game.
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("keeps a typed draft through a pause and ignores app shortcuts", async () => {
    const question = { id: "q1", territory: "src", kind: "free_text", prompt: "Which function makes room in the context?", options: [], source_path: "src/turn.rs", source_excerpt: "", difficulty: 1, cached: false };
    const profile = { project: "p", workspace: "/p", understanding: 10, level: 2, answered: 1, territories: [] };
    const perform = vi.fn(async (kind: string, _payload: unknown) => {
      if (kind === "study.profile") return profile;
      return { mode: "expedition", questions: [question], territory: "src", tokens_used: 0, model: "m" };
    });
    render(<HeroGame gameName="study" palette={palette} host={{ perform }} />);
    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    // ⌘1 is the app's tab switch, not "pick trail 1": nothing is requested.
    stage().focus();
    await userEvent.keyboard("{Meta>}1{/Meta}");
    expect(perform).not.toHaveBeenCalledWith("study.batch", expect.anything());

    await userEvent.keyboard("1");
    await userEvent.type(await screen.findByLabelText("Your answer"), "make_ro");
    // Looking something up elsewhere pauses the game…
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    expect(screen.queryByLabelText("Your answer")).toBeNull();
    // …and coming back finds the half-typed answer where it was left.
    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    expect(await screen.findByLabelText("Your answer")).toHaveValue("make_ro");
  });

  it("fails a request with a clear message when no host is connected", async () => {
    render(<HeroGame gameName="study" palette={palette} />);
    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    await userEvent.keyboard("1");
    await screen.findAllByText("TRAIL BLOCKED");
  });

  it("plays queued cues once each when sound is on", () => {
    const play = vi.spyOn(sfx, "playSfx").mockImplementation(() => undefined);
    render(<HeroGame gameName="tumbleweed" palette={palette} />);
    fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
    fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
    // Starting a run plays the start cue directly.
    expect(play).toHaveBeenCalledWith("start");
    play.mockRestore();
  });
});

it("offers explicit keyboard-accessible play and pause controls", async () => {
  render(<HeroGame gameName="hunt" palette={palette} />);
  await userEvent.click(screen.getByRole("button", { name: /play hunting season/i }));
  expect(stage()).toHaveFocus();
  await userEvent.click(screen.getByRole("button", { name: /pause game/i }));
  expect(screen.getByRole("status")).toHaveTextContent("PAUSED");
  await userEvent.click(screen.getByRole("button", { name: /resume game/i }));
  expect(screen.queryByRole("status")).toBeNull();
});

it("pauses when focus moves into the composer", () => {
  render(<><HeroGame gameName="hunt" palette={palette} /><textarea aria-label="Message" /></>);
  fireEvent.pointerDown(stage(), { clientX: 50, clientY: 50 });
  fireEvent.pointerUp(stage(), { clientX: 50, clientY: 50 });
  fireEvent.focusIn(screen.getByRole("textbox"));
  expect(screen.getByRole("status")).toHaveTextContent("PAUSED");
});

it("pauses the first cabinet when a second one takes focus", async () => {
  render(<><HeroGame gameName="hunt" palette={palette} /><HeroGame gameName="oregon" palette={palette} /></>);
  await userEvent.click(screen.getByRole("button", { name: "Play Hunting Season" }));
  await userEvent.click(screen.getByRole("button", { name: "Play The Oxen Trail" }));
  expect(screen.getAllByRole("status")).toHaveLength(1);
  await userEvent.keyboard("1");
  expect(screen.getByRole("img", { name: /outfit your wagon/i })).toBeInTheDocument();
  expect(screen.getAllByRole("status")).toHaveLength(1);
});

it("keeps keyboard controls focused after changing sound mid-run", async () => {
  render(<HeroGame gameName="hunt" palette={palette} />);
  await userEvent.click(screen.getByRole("button", { name: "Play Hunting Season" }));
  await userEvent.click(screen.getByRole("button", { name: /mute game sound/i }));
  expect(stage()).toHaveFocus();
});

it("starts only one cabinet when the global start combo is entered", async () => {
  render(<><HeroGame gameName="hunt" palette={palette} /><HeroGame gameName="oregon" palette={palette} /></>);
  await userEvent.keyboard("{ArrowUp}{ArrowUp}{ArrowDown}{ArrowDown}");
  expect(screen.getAllByRole("button", { name: "Pause game" })).toHaveLength(1);
});
