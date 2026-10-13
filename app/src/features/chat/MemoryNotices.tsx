// Memory notices above the composer: a local model that loaded into a tight
// machine (fades on its own), one that was refused, or one the watchdog
// stopped to protect the computer (both stay until dismissed — the user
// needs to act: close an app, pick a smaller model). Each notice names the
// apps holding the most memory and offers to quit them, one confirm away.

import { useEffect, useState } from "react";
import { AlertTriangle, OctagonX, X } from "lucide-react";
import { gb, useMemory } from "../../lib/memory";
import { quitMemoryHog } from "../../lib/ipc";
import type { MemoryHog, MemoryNotice } from "../../lib/types";
import { Button, IconButton } from "../../components/ui";

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
  const hogs = notice.hogs ?? [];
  useEffect(() => {
    // A warning fades on its own — unless it offers something to act on.
    if (notice.level !== "warn" || hogs.length > 0) return;
    const t = setTimeout(() => dismiss(notice), WARN_FADE_MS);
    return () => clearTimeout(t);
  }, [notice, hogs.length, dismiss]);
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
        <span className="memory-notice-body">{noticeBody(notice)}</span>
        {hogs.length > 0 && <HogOffers hogs={hogs} />}
      </div>
      <IconButton size="sm" className="memory-notice-close" aria-label="Dismiss" onClick={() => dismiss(notice)}>
        <X size={14} />
      </IconButton>
    </div>
  );
}

/** The message minus what the banner already says: the title's lead-in
 *  ("Stopped <model>…", "memory is tight"), and the "using the most"
 *  sentence the buttons replace. */
export function noticeBody(notice: MemoryNotice): string {
  let body = notice.message;
  for (const lead of [`Stopped ${notice.model} to protect your computer: `, "memory is tight: "]) {
    if (body.startsWith(lead)) body = body.slice(lead.length);
  }
  if ((notice.hogs ?? []).length > 0) {
    body = body
      .replace(/\s*Using the most: .*? GB\)\./, "")
      .replace(/\s*Quitting .+? \(\d+\.\d GB\) frees the most\./, "");
  }
  body = body.trim();
  // Sentence case, but never "MacOS".
  return body.startsWith("macOS") ? body : body.charAt(0).toUpperCase() + body.slice(1);
}

type Outcome = { kind: "quit" | "error"; text: string };

function HogOffers({ hogs }: { hogs: MemoryHog[] }) {
  const refresh = useMemory((s) => s.refresh);
  const [confirming, setConfirming] = useState<MemoryHog | null>(null);
  const [busy, setBusy] = useState(false);
  const [quit, setQuit] = useState<string[]>([]);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const offered = hogs.filter((h) => !quit.includes(h.name));

  async function confirmQuit(hog: MemoryHog) {
    setBusy(true);
    try {
      const freed = await quitMemoryHog(hog.name);
      setQuit((q) => [...q, hog.name]);
      setOutcome({ kind: "quit", text: `${verb(hog, true)} ${hog.name} — about ${gb(freed)} GB freed.` });
      void refresh();
    } catch (err) {
      setOutcome({ kind: "error", text: String(err) });
    } finally {
      setBusy(false);
      setConfirming(null);
    }
  }

  return (
    <>
      {confirming ? (
        <div className="memory-notice-hogs">
          <span className="memory-hog-confirm">
            {confirming.bundle
              ? `Quit ${confirming.name}? It can still ask you to save first.`
              : `Stop every ${confirming.name} process? Unsaved work in them is lost.`}
          </span>
          <Button size="sm" variant="danger" disabled={busy} onClick={() => confirmQuit(confirming)}>
            {verb(confirming, false)} {confirming.name}
          </Button>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirming(null)}>
            Cancel
          </Button>
        </div>
      ) : (
        offered.length > 0 && (
          <div className="memory-notice-hogs">
            <span className="memory-notice-hogs-label">Using the most memory:</span>
            {offered.map((hog) => (
              <Button
                key={hog.name}
                size="sm"
                variant="outline"
                title={`${verb(hog, false)} ${hog.name} to free about ${gb(hog.bytes)} GB`}
                onClick={() => {
                  setOutcome(null);
                  setConfirming(hog);
                }}
              >
                {verb(hog, false)} {hog.name} <span className="memory-hog-size">{gb(hog.bytes)} GB</span>
              </Button>
            ))}
          </div>
        )
      )}
      {outcome && <span className={`memory-notice-result ${outcome.kind}`}>{outcome.text}</span>}
    </>
  );
}

/** An app is quit (it may ask to save); a bare process is stopped. */
function verb(hog: MemoryHog, past: boolean): string {
  if (hog.bundle) return "Quit";
  return past ? "Stopped" : "Stop";
}
