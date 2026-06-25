// Vitest setup: jest-dom matchers, a couple of jsdom polyfills the app touches,
// and per-test cleanup. Store/IPC resets live in `utils.tsx` (imported by test
// files) so they run with the per-file `vi.mock` of `lib/ipc` active.
import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// jsdom lacks matchMedia; the store reads it to pick an initial mode.
if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

// ThemesModal's "Export" copies to the clipboard.
if (!navigator.clipboard) {
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText: async () => {} },
    configurable: true,
  });
}

afterEach(() => cleanup());
