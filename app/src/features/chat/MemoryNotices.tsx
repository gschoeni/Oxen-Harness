// Memory notices above the composer: a local model that loaded into a tight
// machine (fades on its own), one that was refused, or one the watchdog
// stopped to protect the computer (both stay until dismissed — the user
// needs to act: close an app, pick a smaller model). Each notice names the
// apps holding the most memory and offers to quit them, one confirm away.

import { useEffect, useState } from "react";
import { AlertTriangle, AppWindow, OctagonX, SquareTerminal, X } from "lucide-react";
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
  // Sentence case, but never "MacOS" or a lowercase model id ("Gemma-4-…").
  if (body.startsWith("macOS") || body.startsWith(notice.model)) return body;
  return body.charAt(0).toUpperCase() + body.slice(1);
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

  if (offered.length === 0 && !outcome) return null;
  // Bars are scaled to the biggest app, so the list reads at a glance.
  const peak = Math.max(1, ...offered.map((h) => h.bytes));
  return (
    <div className="memory-hogs">
      {offered.length > 0 && (
        <>
          <div className="memory-hogs-head">Using the most memory</div>
          <ul className="memory-hogs-list">
            {offered.map((hog) => {
              const asking = confirming?.name === hog.name;
              const Icon = hog.bundle ? AppWindow : SquareTerminal;
              return (
                <li key={hog.name} className="memory-hog" data-confirming={asking || undefined}>
                  <Icon size={14} strokeWidth={1.75} className="memory-hog-icon" aria-hidden />
                  <span className="memory-hog-name">{hog.name}</span>
                  {asking ? (
                    <span className="memory-hog-ask">
                      {hog.bundle ? "It can still ask you to save first." : "Unsaved work in it is lost."}
                    </span>
                  ) : (
                    <>
                      <span className="memory-hog-bar" aria-hidden>
                        <span style={{ width: `${(hog.bytes / peak) * 100}%` }} />
                      </span>
                      <span className="memory-hog-size">{gb(hog.bytes)} GB</span>
                    </>
                  )}
                  <span className="memory-hog-actions">
                    {asking ? (
                      <>
                        <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirming(null)}>
                          Cancel
                        </Button>
                        <Button size="sm" variant="danger" disabled={busy} onClick={() => confirmQuit(hog)}>
                          {verb(hog, false)} {hog.name}
                        </Button>
                      </>
                    ) : (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={busy}
                        aria-label={`${verb(hog, false)} ${hog.name}, ${gb(hog.bytes)} GB`}
                        onClick={() => {
                          setOutcome(null);
                          setConfirming(hog);
                        }}
                      >
                        {verb(hog, false)}
                      </Button>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </>
      )}
      {outcome && <span className={`memory-notice-result ${outcome.kind}`}>{outcome.text}</span>}
    </div>
  );
}

/** An app is quit (it may ask to save); a bare process is stopped. */
function verb(hog: MemoryHog, past: boolean): string {
  if (hog.bundle) return "Quit";
  return past ? "Stopped" : "Stop";
}
