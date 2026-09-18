// The strip's keyboard, installed once at the app root (like the dock
// shortcuts): ⌘T new chat · ⌘W close · ⌃Tab / ⌃⇧Tab cycle · ⌘1–8 jump,
// ⌘9 last · ⌘K the history. The chat ones stand down while Home, Settings,
// or the history covers the chat; ⌘K works anywhere but Settings.

import { useEffect } from "react";
import { useStore } from "../../lib/store";

export function useChatTabShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      const plainMod = (e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey;
      const key = e.key.toLowerCase();
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
      } else if (e.ctrlKey && e.key === "Tab") {
        e.preventDefault();
        const i = ids.indexOf(current);
        if (i < 0 || ids.length < 2) return;
        void s.resume(ids[(i + (e.shiftKey ? -1 : 1) + ids.length) % ids.length]);
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
