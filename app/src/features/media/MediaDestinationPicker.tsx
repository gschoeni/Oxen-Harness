// The Gallery header's "Saving to <repo>" control: names the Oxen repository
// the project's assets go to and opens a compact `RepositoryBrowser` to
// switch it. Settings → Connection shows the same setting inline
// (`ProjectRepositorySection`); `/repository` switches it from a chat.

import { useEffect, useState } from "react";
import { ChevronDown, CloudUpload, ExternalLink, RotateCcw } from "lucide-react";
import { Button } from "../../components/ui";
import { useMenuState } from "../../components/ui/Menu";
import { openExternal } from "../../lib/ipc";
import { useStore } from "../../lib/store";
import type { MediaDestination } from "../../lib/types";
import { RepositoryBrowser } from "./RepositoryBrowser";
import "./media.css";

export { asRepo } from "./RepositoryBrowser";

export const SOURCE_LABEL: Record<MediaDestination["source"], string> = {
  project: "Chosen for this project",
  project_remote: "From the project's remote repository",
  global: "The default from Settings → Media",
  hub_default: "Oxen's default — your account's playground",
};

export function MediaDestinationPicker({ root }: { root: string }) {
  const destination = useStore((s) => s.mediaDestinations[root]);
  const lookupError = useStore((s) => s.mediaDestinationErrors[root]);
  const refresh = useStore((s) => s.refreshMediaDestination);
  const choose = useStore((s) => s.chooseMediaDestination);
  const { open, setOpen, ref } = useMenuState();
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refresh(root);
  }, [root, refresh]);

  // Unknown is said as unknown: a failed lookup never reads as a destination.
  const label = destination?.repo ?? (lookupError ? "repository unknown" : "your playground");

  return (
    <div className="media-dest" ref={ref}>
      <Button
        type="button"
        size="sm"
        variant="ghost"
        className="picker-btn media-dest-btn"
        onClick={() => {
          setError(null);
          // Opening after a failed lookup also tries it again.
          if (!open && lookupError) void refresh(root);
          setOpen((o) => !o);
        }}
        title={
          destination
            ? `${label} — ${SOURCE_LABEL[destination.source]}`
            : lookupError
              ? `Couldn't reach Oxen.ai: ${lookupError}`
              : "The Oxen repository this project's assets go to"
        }
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={`Repository: ${label}`}
      >
        <CloudUpload size={13} aria-hidden="true" />
        <span className="picker-label">{label}</span>
        <ChevronDown size={13} className="picker-caret" />
      </Button>

      {open && (
        <div className="media-dest-pop" role="dialog" aria-label="Project repository">
          <RepositoryBrowser
            root={root}
            current={destination?.repo ?? null}
            layout="compact"
            onDone={() => setOpen(false)}
          />
          {error && (
            <div className="repo-browser-error" role="alert">
              {error}
            </div>
          )}
          {(destination?.source === "project" || destination?.web_url) && (
            <div className="media-dest-foot">
              {destination?.source === "project" && (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    choose(root, null).then(
                      () => setOpen(false),
                      (e) => setError(String(e)),
                    )
                  }
                >
                  <RotateCcw size={13} /> Use the default
                </Button>
              )}
              {destination?.web_url && (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setOpen(false);
                    if (destination.web_url) void openExternal(destination.web_url);
                  }}
                >
                  <ExternalLink size={13} /> Open on Oxen.ai
                </Button>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
