// Settings → Connection's project-scoped block: the Oxen repository the open
// project's assets go to on the hub (today its generated images and videos).
// Same picker and same setting as the Gallery's "Saving to" and
// `/repository`; this is the place to find it without going looking.

import { FolderOpen } from "lucide-react";
import { useActiveProject, useStore } from "../../lib/store";
import { MediaDestinationPicker, SOURCE_LABEL } from "../media/MediaDestinationPicker";

export function ProjectRepositorySection() {
  const project = useActiveProject();
  const destination = useStore((s) => (project ? s.mediaDestinations[project.path] : undefined));

  return (
    <section className="settings-section" aria-label="Project repository">
      <div className="settings-label settings-label-scoped">
        Project repository
        {project && (
          <span className="settings-scope" title={project.path}>
            <FolderOpen size={12} aria-hidden="true" />
            {project.name}
          </span>
        )}
      </div>
      {project ? (
        <>
          <div className="project-repository-row">
            <MediaDestinationPicker root={project.path} />
            {destination && <span className="project-repository-source">{SOURCE_LABEL[destination.source]}</span>}
          </div>
          <p className="hint">
            Where <strong>{project.name}</strong> keeps its assets on Oxen.ai — today, every image and
            video generated in it. Pick any repository you can write to, or type a new name in your own
            namespace or an organization you edit and it's created on first use. Applies to this project
            only, including chats already open. Also: <code>/repository</code> in the chat.
          </p>
        </>
      ) : (
        <p className="hint">Open a project to choose the repository its assets go to.</p>
      )}
    </section>
  );
}
