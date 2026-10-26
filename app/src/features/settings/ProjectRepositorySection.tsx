// Settings → Connection's project-scoped block: the Oxen repository the open
// project's assets go to on the hub (today its generated images and videos).
// Same setting as the Gallery's "Saving to" and `/repository`; here "Change"
// opens the full two-column browser inline, beneath the card.

import { useEffect, useState } from "react";
import { ChevronDown, ChevronUp, CloudUpload, ExternalLink, FolderOpen, RotateCcw } from "lucide-react";
import { Button, IconButton } from "../../components/ui";
import { openExternal } from "../../lib/ipc";
import { useActiveProject, useStore } from "../../lib/store";
import { SOURCE_LABEL } from "../media/MediaDestinationPicker";
import { RepositoryBrowser } from "../media/RepositoryBrowser";

export function ProjectRepositorySection() {
  const project = useActiveProject();
  return (
    <section className="settings-section" aria-label="Project repository">
      <div className="settings-section-head">
        <h3 className="settings-heading">
          Project repository
          {project && (
            <span className="settings-scope" title={project.path}>
              <FolderOpen size={12} aria-hidden="true" />
              <span className="settings-scope-name">{project.name}</span>
            </span>
          )}
        </h3>
        <p className="settings-blurb">
          {project
            ? "Where this project's assets are stored on Oxen.ai — today, every image and video generated in it. Applies to this project only, including chats already open."
            : "Open a project to choose the repository its assets go to."}
        </p>
      </div>
      {/* Keyed by project: switching projects starts from a closed browser
          with no leftover query or error. */}
      {project && <ProjectRepository key={project.path} root={project.path} />}
    </section>
  );
}

function ProjectRepository({ root }: { root: string }) {
  const destination = useStore((s) => s.mediaDestinations[root]);
  const lookupError = useStore((s) => s.mediaDestinationErrors[root]);
  const refresh = useStore((s) => s.refreshMediaDestination);
  const choose = useStore((s) => s.chooseMediaDestination);
  const [browsing, setBrowsing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refresh(root);
  }, [root, refresh]);

  const origin = destination
    ? SOURCE_LABEL[destination.source]
    : lookupError
      ? `Couldn't reach Oxen.ai: ${lookupError}`
      : "Looking it up…";

  return (
    <>
      <div className={`project-repository ${browsing ? "browsing" : ""}`}>
        <div className="project-repository-card">
          <CloudUpload size={18} className="project-repository-icon" aria-hidden="true" />
          <div className="project-repository-text">
            <span className="project-repository-name">{destination?.repo ?? "Your playground"}</span>
            <span className={`project-repository-origin ${lookupError && !destination ? "error" : ""}`}>{origin}</span>
          </div>
          {lookupError && !destination && (
            <Button type="button" size="sm" variant="ghost" onClick={() => void refresh(root)}>
              Retry
            </Button>
          )}
          {destination?.source === "project" && !browsing && (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              title="Go back to the project remote, the Settings → Media default, or your playground"
              onClick={() => {
                setError(null);
                choose(root, null).catch((e) => setError(String(e)));
              }}
            >
              <RotateCcw size={13} /> Use default
            </Button>
          )}
          {destination?.web_url && (
            <IconButton
              aria-label="Open the repository on Oxen.ai"
              title={destination.web_url}
              onClick={() => void openExternal(destination.web_url ?? undefined)}
            >
              <ExternalLink size={15} />
            </IconButton>
          )}
          <Button
            type="button"
            size="sm"
            variant={browsing ? "ghost" : "outline"}
            aria-expanded={browsing}
            onClick={() => {
              setError(null);
              setBrowsing((b) => !b);
            }}
          >
            {browsing ? "Done" : "Change"}
            {browsing ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
          </Button>
        </div>
        {browsing && (
          <RepositoryBrowser
            root={root}
            current={destination?.repo ?? null}
            layout="columns"
            onDone={() => setBrowsing(false)}
          />
        )}
      </div>
      {error && (
        <div className="repo-browser-error" role="alert">
          {error}
        </div>
      )}
      {!browsing && (
        <p className="hint">
          Pick any repository you can write to, or name a new one in your own namespace or an
          organization you edit — it's created on first use. In a chat: <code>/repository</code>.
        </p>
      )}
    </>
  );
}
