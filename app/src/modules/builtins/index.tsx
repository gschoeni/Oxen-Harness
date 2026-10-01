import { useEffect } from "react";
import { FileText, GraduationCap, Images, PanelTop, PanelsTopLeft, Globe } from "lucide-react";
import { useStore } from "../../lib/store";
import { useDocument, type ViewProps } from "../../workbench-sdk";
import type { CanvasDoc } from "../../lib/types";
import type { ViewModule } from "../../workbench-sdk";
import { Canvas, CanvasView } from "../../features/canvas/Canvas";
import { Preview } from "../../features/preview/Preview";
import { Browser } from "../../features/browser/Browser";
import { GalleryPanel } from "../../features/media/GalleryPanel";
import { EditorPane } from "../../features/files/EditorPane";
import { StudyView } from "../../features/chat/StudyView";
import { STUDY_VIEW } from "../../lib/store";

export default [
  {
    id: "editor",
    title: "File",
    icon: FileText,
    description: "Read and edit project files.",
    priority: -100,
    matches: () => true,
    component: Files,
  },
  {
    id: "gallery",
    title: "Gallery",
    icon: Images,
    description: "Browse generated images and videos.",
    component: () => <GalleryPanel />,
  },
  {
    id: "preview",
    title: "Preview",
    icon: PanelTop,
    description: "See your running website.",
    component: () => <Preview />,
  },
  {
    id: "canvas",
    title: "Canvas",
    icon: PanelsTopLeft,
    description: "Documents made by the agent.",
    priority: 90,
    matches: (path: string) => path.endsWith(".canvas.json"),
    component: CanvasModule,
  },
  {
    id: "browser",
    title: "Browser",
    icon: Globe,
    description: "Explore links from your conversation.",
    component: () => <Browser />,
  },
  {
    id: STUDY_VIEW,
    title: "Study",
    icon: GraduationCap,
    description: "Learn the code your agent is writing.",
    component: () => <StudyView />,
  },
] satisfies ViewModule[];

function Files({ api }: ViewProps) {
  const { session, target } = api.context;
  const pane = useStore((s) => s.editorTabs[session]);
  useEffect(() => {
    if ((target.path || target.paths) && !pane?.tabs.length)
      useStore.getState().openWorkView(session, target);
  }, [session, target, pane]);
  return pane?.tabs.length ? (
    <EditorPane />
  ) : (
    <div className="workbench-welcome">
      <p>Open a file from the project tree to start editing.</p>
    </div>
  );
}

function CanvasModule({ api }: ViewProps) {
  return api.context.target.path?.endsWith(".canvas.json") ? <CanvasFile api={api} /> : <Canvas />;
}
function CanvasFile({ api }: ViewProps) {
  const document = useDocument(api, api.context.target.path!);
  if (!document.snapshot)
    return <p className="workbench-error">{document.error ?? "Loading canvas…"}</p>;
  try {
    const doc = JSON.parse(document.content) as CanvasDoc;
    if (typeof doc.content !== "string" || typeof doc.format !== "string")
      throw new Error("Canvas needs content and format");
    return (
      <div className="canvas">
        <header className="canvas-head">
          <strong>{doc.title}</strong>
          <button onClick={() => api.open({ view: "editor", path: api.context.target.path })}>
            Edit source
          </button>
        </header>
        <div className="canvas-body">
          <CanvasView doc={doc} />
        </div>
      </div>
    );
  } catch (e) {
    return (
      <p role="alert" className="workbench-error">
        {String(e)}
      </p>
    );
  }
}
