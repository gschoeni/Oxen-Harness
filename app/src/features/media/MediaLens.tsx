// Home's media lens: every project that has generations, newest first, as
// one section per project. Loads each project's library on mount (in
// parallel, tolerating failures) and follows `media://changed` after that.
// A tile enters that project and opens the Gallery on it.

import { useEffect, useMemo, useState } from "react";
import { Images } from "lucide-react";
import { useStore } from "../../lib/store";
import type { MediaItem, Project } from "../../lib/types";
import { mediaCountLabel, mediaForRoot, mediaRoot } from "../../lib/media";
import { MediaTile } from "./MediaTile";
import "./media.css";

type Filter = "all" | "image" | "video";
const PER_PROJECT = 24;

export function MediaLens({ projects }: { projects: Project[] }) {
  const media = useStore((s) => s.media);
  const refreshMedia = useStore((s) => s.refreshMedia);
  const enterProject = useStore((s) => s.enterProject);
  const openGallery = useStore((s) => s.openGallery);
  const [filter, setFilter] = useState<Filter>("all");
  const [loaded, setLoaded] = useState(false);

  const paths = useMemo(() => projects.map((p) => mediaRoot(p.path)), [projects]);
  useEffect(() => {
    let stale = false;
    setLoaded(false);
    Promise.all(paths.map((p) => refreshMedia(p).catch(() => {}))).then(() => {
      if (!stale) setLoaded(true);
    });
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paths.join("\n"), refreshMedia]);

  const sections = useMemo(
    () =>
      projects
        .map((project) => {
          const items = (mediaForRoot(media, project.path) ?? []).filter(
            (i) => i.status !== "cancelled" && (filter === "all" || i.kind === filter),
          );
          return { project, all: mediaForRoot(media, project.path) ?? [], items: items.slice(0, PER_PROJECT) };
        })
        .filter((s) => s.items.length > 0),
    [projects, media, filter],
  );

  async function open(project: Project, item?: MediaItem) {
    await enterProject(project.path);
    openGallery(item?.id);
  }

  return (
    <section className="media-lens" aria-label="Media across projects">
      <div className="media-lens-toolbar">
        <div className="gallery-filters" role="group" aria-label="Filter">
          {(["all", "image", "video"] as Filter[]).map((f) => (
            <button
              key={f}
              type="button"
              className={`gallery-chip ${filter === f ? "on" : ""}`}
              onClick={() => setFilter(f)}
              aria-pressed={filter === f}
            >
              {f === "all" ? "All" : f === "image" ? "Images" : "Videos"}
            </button>
          ))}
        </div>
      </div>
      {sections.length === 0 ? (
        <div className="media-lens-empty">
          <Images size={22} />
          <p>
            {loaded
              ? "No generations yet. Ask a chat for an image or a video and it shows up here."
              : "Looking for generations…"}
          </p>
        </div>
      ) : (
        sections.map(({ project, all, items }) => (
          <section key={project.path} className="media-lens-project" aria-label={project.name}>
            <header className="media-lens-head">
              <button
                type="button"
                className="media-lens-name"
                title={`${project.path} — open gallery`}
                onClick={() => void open(project)}
              >
                {project.name}
              </button>
              <span className="media-lens-count">{mediaCountLabel(all)}</span>
            </header>
            <div className="media-lens-grid">
              {items.map((item) => (
                <MediaTile key={item.id} item={item} workspace={mediaRoot(project.path)} onSelect={() => void open(project, item)} />
              ))}
            </div>
          </section>
        ))
      )}
    </section>
  );
}
