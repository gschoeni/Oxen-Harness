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
