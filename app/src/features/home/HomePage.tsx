// Home — the app's front door: one card per project (wearing its vital
// signs), the media lens across projects, and the way into any project's
// page. Which project page is open is the store's decision.

import { useEffect, useState } from "react";
import { FolderPlus, Images, LayoutGrid, Settings2 } from "lucide-react";
import { Button } from "../../components/ui";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
import { getUi, setUi } from "../../lib/uiState";
import { MediaLens } from "../media/MediaLens";
import { ProjectCards } from "../projects/ProjectCards";
import { ProjectHome } from "../projects/ProjectHome";
import { StartProjectModal } from "../projects/StartProjectModal";
import "./home.css";

export function HomePage() {
  const projects = useStore((s) => s.projects);
  const projectHomePath = useStore((s) => s.projectHomePath);
  const openProjectHome = useStore((s) => s.openProjectHome);
  const setHomeOpen = useStore((s) => s.setHomeOpen);
  const refreshHistory = useStore((s) => s.refreshHistory);
  const refreshThreads = useStore((s) => s.refreshThreads);
  // Which project page is open is the STORE's decision (projectHomePath) —
  // the titlebar and other chrome navigate by setting it while Home is
  // already mounted, so a mount-time snapshot would leave their buttons dead.
  // The local state only mirrors it as a resolved Project object (and carries
  // in-page edits between refreshes).
  const [selected, setSelected] = useState<Project | null>(null);
  useEffect(() => {
    setSelected((current) =>
      projectHomePath
        ? current?.path === projectHomePath
          ? current
          : (projects.find((p) => p.path === projectHomePath) ?? null)
        : null,
    );
  }, [projectHomePath, projects]);
  const [starting, setStarting] = useState(false);

  // The verdicts load themselves: on first mount (app start opens Home
  // without going through setHomeOpen) and whenever the window regains focus.
  useEffect(() => {
    void refreshThreads();
    const onFocus = () => void refreshThreads();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshThreads]);

  async function projectChanged(project: Project) {
    setSelected((current) => ({
      ...project,
      session_count: current?.session_count ?? project.session_count,
      active: true,
    }));
    await refreshHistory();
  }

  return (
    <div className="home-overlay" role="dialog" aria-modal="true" aria-label="Home">
      {selected ? (
        <ProjectHome
          project={selected}
          // Back through the store, so projectHomePath agrees with what's on
          // screen (and the verdicts refresh on return).
          onBack={() => setHomeOpen(true)}
          onProjectChanged={projectChanged}
        />
      ) : (
        <Overview
          onOpenProject={(project) => openProjectHome(project.path)}
          onStartProject={() => setStarting(true)}
        />
      )}

      {starting && (
        <StartProjectModal
          onClose={() => setStarting(false)}
          onCreated={async (project) => {
            setStarting(false);
            await useStore.getState().enterProject(project.path);
          }}
        />
      )}
    </div>
  );
}

/** Home's two lenses: the project cards (the default) and media (every
 *  project's generated images and clips). Persisted — a lens is a habit. */
type HomeView = "cards" | "media";

function savedView(): HomeView {
  return getUi("homeView") === "media" ? "media" : "cards";
}

function Overview({
  onOpenProject,
  onStartProject,
}: {
  onOpenProject: (project: Project) => void;
  onStartProject: () => void;
}) {
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const projects = useStore((s) => s.projects);
  const [view, setView] = useState<HomeView>(savedView);

  function changeView(next: HomeView) {
    setView(next);
    setUi("homeView", next);
  }

  return (
    <main className="home-page">
      <header className="home-header">
        <div>
          <div className="home-eyebrow">{today()}</div>
          <h1 className="home-title">Home</h1>
        </div>
        <div className="home-header-actions">
          <div className="home-view-toggle" role="group" aria-label="Home view">
            <button
              className={`home-view-option ${view === "cards" ? "selected" : ""}`}
              aria-pressed={view === "cards"}
              title="Cards — one card per project"
              onClick={() => changeView("cards")}
            >
              <LayoutGrid size={13} /> cards
            </button>
            <button
              className={`home-view-option ${view === "media" ? "selected" : ""}`}
              aria-pressed={view === "media"}
              title="Media — generated images and clips across projects"
              onClick={() => changeView("media")}
            >
              <Images size={13} /> media
            </button>
          </div>
          <Button variant="ghost" onClick={() => setSettingsOpen(true)}>
            <Settings2 size={16} /> Settings
          </Button>
          <Button variant="ghost" onClick={onStartProject}>
            <FolderPlus size={16} /> New project
          </Button>
        </div>
      </header>

      {view === "media" ? (
        <MediaLens projects={projects} />
      ) : (
        <ProjectCards onOpenProject={onOpenProject} />
      )}
    </main>
  );
}

function today(): string {
  return new Date().toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
  });
}
