// Settings → Connection's project-scoped block: the Oxen repository the open
// project's assets go to on the hub (today its generated images and videos).
// Same picker and same setting as the Gallery's "Saving to" and
// `/repository`; this is the place to find it without going looking.

import { CloudUpload, ExternalLink, FolderOpen } from "lucide-react";
import { IconButton } from "../../components/ui";
import { openExternal } from "../../lib/ipc";
import { useActiveProject, useStore } from "../../lib/store";
import type { MediaDestination } from "../../lib/types";
import { MediaDestinationPicker } from "../media/MediaDestinationPicker";

/** Where the repository in effect came from, said in full. */
const ORIGIN: Record<MediaDestination["source"], string> = {
  project: "Chosen for this project",
  project_remote: "From the project's remote repository (project page)",
  global: "The default from Settings → Media",
  hub_default: "Oxen's default — your account's playground",
};

export function ProjectRepositorySection() {
  const project = useActiveProject();
  const destination = useStore((s) => (project ? s.mediaDestinations[project.path] : undefined));

  return (
    <section className="settings-section" aria-label="Project repository">
      <div className="settings-section-head">
        <h3 className="settings-heading">
          Project repository
          {project && (
            <span className="settings-scope" title={project.path}>
              <FolderOpen size={12} aria-hidden="true" />
              {project.name}
            </span>
          )}
        </h3>
        <p className="settings-blurb">
          {project
            ? "Where this project's assets are stored on Oxen.ai — today, every image and video generated in it. Applies to this project only, including chats already open."
            : "Open a project to choose the repository its assets go to."}
        </p>
      </div>
      {project && (
        <>
          <div className="project-repository-card">
            <CloudUpload size={18} className="project-repository-icon" aria-hidden="true" />
            <div className="project-repository-text">
              <span className="project-repository-name">{destination?.repo ?? "Your playground"}</span>
              <span className="project-repository-origin">
                {destination ? ORIGIN[destination.source] : "Looking it up…"}
              </span>
            </div>
            {destination?.web_url && (
              <IconButton
                aria-label="Open the repository on Oxen.ai"
                title={destination.web_url}
                onClick={() => void openExternal(destination.web_url ?? undefined)}
              >
                <ExternalLink size={15} />
              </IconButton>
            )}
            <MediaDestinationPicker root={project.path} trigger="change" />
          </div>
          <p className="hint">
            Pick any repository you can write to, or type a new name in your own namespace or an
            organization you edit — it's created on first use. In a chat: <code>/repository</code>.
          </p>
        </>
      )}
    </section>
  );
}
