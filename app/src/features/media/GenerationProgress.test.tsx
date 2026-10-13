import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";

import { GenerationProgress, progressLabel, progressPercent } from "./GenerationProgress";
import type { MediaItem, MediaProgress } from "../../lib/types";

const item = (progress: MediaProgress | null, status: MediaItem["status"] = "processing"): MediaItem => ({
  id: "local-1",
  session: "s1",
  turn_seq: null,
  call_id: "c1",
  batch: "b",
  index: 1,
  kind: "image",
  model: "local/iris-3b-turbo",
  prompt: "a fox",
  params: {},
  refs: [],
  path: null,
  poster: null,
  bytes: 0,
  width: null,
  height: null,
  duration_secs: null,
  cost_usd: null,
  status,
  error: null,
  created_at: 0,
  completed_at: null,
  parent: null,
  seed: null,
  sources: [],
  agent_prompt: null,
  provider: null,
  progress,
});

describe("GenerationProgress", () => {
  it("fills by step while sampling", () => {
    const sampling = item({ phase: "sampling", step: 3, total: 4, note: null });
    expect(progressPercent(sampling)).toBe(75);
    render(<GenerationProgress item={sampling} />);
    const bar = screen.getByRole("progressbar", { name: "Step 3 of 4" });
    expect(bar).toHaveAttribute("aria-valuenow", "75");
    expect(screen.getByText("75%")).toBeInTheDocument();
  });

  it("sweeps while loading and shows a memory warning", () => {
    const loading = item({ phase: "loading", step: null, total: null, note: "memory is tight" });
    expect(progressPercent(loading)).toBeNull();
    const { container } = render(<GenerationProgress item={loading} />);
    expect(container.querySelector(".media-upload-track.indeterminate")).not.toBeNull();
    expect(screen.getByText("Loading the model")).toBeInTheDocument();
    expect(screen.getByText("memory is tight")).toBeInTheDocument();
  });

  it("labels hosted jobs, which report no steps, by their status", () => {
    expect(progressLabel(item(null, "queued"))).toBe("Queued");
    expect(progressLabel(item(null))).toBe("Rendering");
    expect(progressLabel(item({ phase: "finishing", step: null, total: null, note: null }))).toBe("Decoding the image");
  });
});
