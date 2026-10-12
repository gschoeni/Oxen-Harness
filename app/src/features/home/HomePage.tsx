// Home — the app's front door: one card per project (wearing its vital
// signs) and the way into any project's page. Which project page is open is the store's decision.

import { useEffect, useState } from "react";
import { FolderPlus, Settings2 } from "lucide-react";
import { Button } from "../../components/ui";
import { useStore } from "../../lib/store";
import type { Project } from "../../lib/types";
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

function Overview({
  onOpenProject,
  onStartProject,
}: {
  onOpenProject: (project: Project) => void;
  onStartProject: () => void;
}) {
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);

  return (
    <main className="home-page">
      <header className="home-header">
        <div>
          <div className="home-eyebrow">{today()}</div>
          <h1 className="home-title">Home</h1>
        </div>
        <div className="home-header-actions">
          <Button variant="ghost" onClick={() => setSettingsOpen(true)}>
            <Settings2 size={16} /> Settings
          </Button>
          <Button variant="ghost" onClick={onStartProject}>
            <FolderPlus size={16} /> New project
          </Button>
        </div>
      </header>

      <ProjectCards onOpenProject={onOpenProject} />
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
