// The work view before this chat has put anything in it. The pane is where
// the agent's work lands — files it opens, previews it starts, images it
// makes — so the empty state asks the agent for one of those rather than
// describing the pane. Each chip is a prompt sent as written; the quiet
// links below are the two things the user can open by hand.

import { useEffect } from "react";
import { FolderTree, Images } from "lucide-react";
import { Button } from "../../components/ui";
import { useStore } from "../../lib/store";
import { mediaForRoot } from "../../lib/media";

/** Sent verbatim: what the chip says is what the agent is asked. */
export const WORK_STARTERS = [
  "Show me how this project is laid out",
  "Run the dev server and open a preview",
  "Generate a hero image for this project",
];

export function WorkEmpty({ workspace }: { workspace: string }) {
  const send = useStore((s) => s.send);
  const setLeftTab = useStore((s) => s.setLeftTab);
  const openGallery = useStore((s) => s.openGallery);
  const refreshMedia = useStore((s) => s.refreshMedia);
  const generations =
    useStore((s) => mediaForRoot(s.media, workspace))?.filter((i) => i.status !== "cancelled").length ?? 0;
  useEffect(() => {
    void refreshMedia(workspace);
  }, [workspace, refreshMedia]);
  return (
    <div className="workbench-welcome work-empty">
      <h2>Nothing open yet.</h2>
      <p>The agent puts files, previews and generations here. Try:</p>
      <ul className="work-starters" aria-label="Ask the agent">
        {WORK_STARTERS.map((prompt) => (
          <li key={prompt}>
            <Button type="button" size="sm" variant="outline" onClick={() => send(prompt)}>
              {prompt}
            </Button>
          </li>
        ))}
      </ul>
      <p className="work-empty-links">
        <span>or</span>
        <Button type="button" size="sm" variant="ghost" onClick={() => setLeftTab("files")}>
          <FolderTree size={13} aria-hidden="true" />
          Open a file…
        </Button>
        {generations > 0 && (
          <>
            <span aria-hidden="true">·</span>
            <Button type="button" size="sm" variant="ghost" onClick={() => openGallery()}>
              <Images size={13} aria-hidden="true" />
              Gallery · {generations}
            </Button>
          </>
        )}
      </p>
    </div>
  );
}
