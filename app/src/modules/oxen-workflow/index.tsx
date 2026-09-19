import type { ViewModule } from "../../workbench-sdk";
import { lazy } from "react";
const WorkflowView = lazy(() =>
  import("./WorkflowView").then((module) => ({ default: module.WorkflowView })),
);
export default [
  {
    id: "workflow",
    title: "Oxen workflow",
    description: "Connect prompts, images, video, and upscaling.",
    priority: 100,
    matches: (path: string) => path.toLowerCase().endsWith(".graph.json"),
    component: WorkflowView,
  },
] satisfies ViewModule[];
