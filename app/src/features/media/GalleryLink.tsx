// The standing way into the Gallery: a button in the work view's header,
// there whenever the project has generated anything, plus a pill naming
// what is still rendering that opens the Gallery on exactly those (each
// with its cancel). The right-hand work view only shows what has been opened
// in it, so without this the Gallery could only be reached through a chat
// card that happened to be on screen.

import { useEffect } from "react";
import { Images } from "lucide-react";
import { useStore } from "../../lib/store";
import { isInFlight, mediaForRoot } from "../../lib/media";
import "./media.css";

export function GalleryLink({ workspace }: { workspace: string }) {
  const items = useStore((s) => mediaForRoot(s.media, workspace));
  const refreshMedia = useStore((s) => s.refreshMedia);
  const openGallery = useStore((s) => s.openGallery);
  const openRendering = useStore((s) => s.openRendering);
  const showing = useStore((s) =>
    s.session ? s.workContexts[s.session.session_id]?.current.view === "gallery" : false,
  );
  useEffect(() => {
    void refreshMedia(workspace);
  }, [workspace, refreshMedia]);

  const count = items?.filter((i) => i.status !== "cancelled").length ?? 0;
  if (count === 0) return null;
  const rendering = items?.filter(isInFlight).length ?? 0;
  return (
    <>
      {rendering > 0 && (
        <button
          type="button"
          className="gallery-link gallery-link-busy"
          title="Show what is still rendering (and cancel it)"
          onClick={() => openRendering()}
        >
          <span className="gallery-link-dot" aria-hidden="true" />
          {rendering} rendering
        </button>
      )}
      <button
        type="button"
        className={`gallery-link${showing ? " on" : ""}`}
        aria-current={showing ? "page" : undefined}
        title="Browse this project's generated images and videos"
        onClick={() => openGallery()}
      >
        <Images size={14} aria-hidden="true" />
        Gallery
        <span className="gallery-link-count">{count}</span>
      </button>
    </>
  );
}
