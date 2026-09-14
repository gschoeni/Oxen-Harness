// The project page's media card: the newest generations as a strip of
// tiles, the count, and the way into the full Gallery. Cold-loads the
// project's library and follows `media://changed` after that.

import { useEffect, useMemo } from "react";
import { Images } from "lucide-react";
import { Button } from "../../components/ui";
import { useStore } from "../../lib/store";
import { mediaCountLabel, mediaForRoot, mediaRoot } from "../../lib/media";
import { MediaTile } from "./MediaTile";
import "./media.css";

const STRIP = 8;

export function ProjectMediaCard({ path }: { path: string }) {
  const root = mediaRoot(path);
  const items = useStore((s) => mediaForRoot(s.media, root));
  const refreshMedia = useStore((s) => s.refreshMedia);
  const enterProject = useStore((s) => s.enterProject);
  const openGallery = useStore((s) => s.openGallery);
  useEffect(() => {
    void refreshMedia(root);
  }, [root, refreshMedia]);

  const shown = useMemo(
    () => (items ?? []).filter((i) => i.status !== "cancelled" && i.status !== "failed" && i.status !== "timed_out").slice(0, STRIP),
    [items],
  );
  const count = mediaCountLabel(items ?? []);

  async function open(itemId?: string) {
    await enterProject(path);
    openGallery(itemId);
  }

  return (
    <section className="project-context-card project-media-card" aria-label="Media">
      <div className="project-context-card-header">
        <div>
          <h2>Media</h2>
          <p>{shown.length ? count : "Generated images and clips."}</p>
        </div>
        {shown.length > 0 && (
          <Button variant="ghost" onClick={() => void open()}>
            <Images size={14} /> Open gallery
          </Button>
        )}
      </div>
      {shown.length === 0 ? (
        <p className="project-media-empty">No generations yet — ask a chat to make one.</p>
      ) : (
        <div className="project-media-strip" role="list">
          {shown.map((item) => (
            <div key={item.id} className="project-media-slot" role="listitem">
              <MediaTile item={item} workspace={root} onSelect={() => void open(item.id)} />
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
