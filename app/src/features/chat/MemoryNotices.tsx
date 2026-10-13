// Memory notices above the composer: a local model that loaded into a tight
// machine (fades on its own), one that was refused, or one the watchdog
// stopped to protect the computer (both stay until dismissed — the user
// needs to act: close an app, pick a smaller model).

import { useEffect } from "react";
import { AlertTriangle, OctagonX, X } from "lucide-react";
import { useMemory } from "../../lib/memory";
import type { MemoryNotice } from "../../lib/types";

const WARN_FADE_MS = 12_000;

export function MemoryNotices() {
  const banners = useMemory((s) => s.banners);
  if (banners.length === 0) return null;
  return (
    <div className="memory-notices" role="status" aria-live="polite">
      {banners.map((n) => (
        <Notice key={`${n.model}-${n.level}-${n.at}`} notice={n} />
      ))}
    </div>
  );
}

function Notice({ notice }: { notice: MemoryNotice }) {
  const dismiss = useMemory((s) => s.dismiss);
  useEffect(() => {
    if (notice.level !== "warn") return;
    const t = setTimeout(() => dismiss(notice), WARN_FADE_MS);
    return () => clearTimeout(t);
  }, [notice, dismiss]);
  const Icon = notice.level === "warn" ? AlertTriangle : OctagonX;
  const title =
    notice.level === "stopped"
      ? `Stopped ${notice.model}`
      : notice.level === "refused"
        ? `Didn't load ${notice.model}`
        : "Memory is tight";
  return (
    <div className={`memory-notice ${notice.level}`}>
      <Icon size={15} className="memory-notice-icon" aria-hidden />
      <div className="memory-notice-text">
        <span className="memory-notice-title">{title}</span>
        <span className="memory-notice-body">{notice.message}</span>
      </div>
      <button
        type="button"
        className="memory-notice-close"
        aria-label="Dismiss"
        onClick={() => dismiss(notice)}
      >
        <X size={13} />
      </button>
    </div>
  );
}
