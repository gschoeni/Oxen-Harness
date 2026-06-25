import { FolderOpen, FolderPlus } from "lucide-react";
import { Modal } from "../../components/ui";
import { useStore } from "../../lib/store";
import "./projects.css";

/** List / create / enter projects. A project is a working directory the agent
 *  runs in; its chats are grouped under it in the sidebar. */
export function ProjectsModal() {
  const projects = useStore((s) => s.projects);
  const activePath = useStore((s) => s.session?.workspace ?? null);
  const enterProject = useStore((s) => s.enterProject);
  const createProject = useStore((s) => s.createProject);
  const setProjectsOpen = useStore((s) => s.setProjectsOpen);

  return (
    <Modal title="Projects" onClose={() => setProjectsOpen(false)} wide>
      <p className="projects-intro">
        A project is a folder the agent works in. Open one to chat against that codebase — each
        project keeps its own chats.
      </p>

      <button className="project-card new" onClick={() => createProject()}>
        <FolderPlus size={18} />
        <span className="project-card-main">
          <span className="project-card-name">Open a folder…</span>
          <span className="project-card-path">Add a project from your computer</span>
        </span>
      </button>

      <div className="projects-list">
        {projects.map((p) => (
          <button
            key={p.path}
            className={`project-card ${p.path === activePath ? "active" : ""}`}
            onClick={() => enterProject(p.path)}
          >
            <FolderOpen size={18} />
            <span className="project-card-main">
              <span className="project-card-name">
                {p.name}
                {p.path === activePath && <span className="project-card-badge">current</span>}
              </span>
              <span className="project-card-path" title={p.path}>
                {p.path}
              </span>
            </span>
            <span className="project-card-count">
              {p.session_count} chat{p.session_count === 1 ? "" : "s"}
            </span>
          </button>
        ))}
      </div>
    </Modal>
  );
}
