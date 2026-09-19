// The strip's keyboard, installed once at the app root (like the dock
// shortcuts): ⌘T new chat · ⌘W close · ⌃Tab / ⌃⇧Tab and ⌘⇧→ / ⌘⇧← cycle ·
// ⌘1–8 jump, ⌘9 last · ⌘K the history. The chat ones stand down while Home, Settings,
// or the history covers the chat; ⌘K works anywhere but Settings.

import { useEffect } from "react";
import { useStore } from "../../lib/store";

export function useChatTabShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      const mod = e.metaKey || e.ctrlKey;
      const plainMod = mod && !e.shiftKey && !e.altKey;
      const key = e.key.toLowerCase();
      // ⌃Tab / ⌃⇧Tab and ⌘⇧→ / ⌘⇧← both walk the strip, wrapping at the ends.
      const cycle =
        e.ctrlKey && e.key === "Tab"
          ? (e.shiftKey ? -1 : 1)
          : mod && e.shiftKey && !e.altKey && (e.key === "ArrowRight" || e.key === "ArrowLeft")
            ? (e.key === "ArrowRight" ? 1 : -1)
            : 0;
      if (plainMod && key === "k" && !s.settingsOpen) {
        e.preventDefault();
        s.setHistoryOpen(!s.historyOpen);
        return;
      }
      if (s.homeOpen || s.settingsOpen || s.historyOpen || !s.session) return;
      const ids = s.chatTabs[s.session.workspace] ?? [];
      const current = s.session.session_id;
      if (plainMod && key === "t") {
        e.preventDefault();
        void s.newChat();
      } else if (plainMod && key === "w") {
        e.preventDefault();
        void s.closeTab(current);
      } else if (cycle !== 0) {
        e.preventDefault();
        const i = ids.indexOf(current);
        if (i < 0 || ids.length < 2) return;
        void s.resume(ids[(i + cycle + ids.length) % ids.length]);
      } else if (plainMod && /^[1-9]$/.test(e.key)) {
        const target = e.key === "9" ? ids[ids.length - 1] : ids[Number(e.key) - 1];
        if (!target) return;
        e.preventDefault();
        void s.resume(target);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
