import { lazy } from "react";
import type { ViewModule } from "../../workbench-sdk";
const ViewStudio = lazy(() =>
  import("./ViewStudio").then((module) => ({ default: module.ViewStudio })),
);
export default [
  {
    id: "view-studio",
    title: "View Studio",
    description: "Build your own views with your agent. Preview, test, and install live.",
    component: ViewStudio,
  },
] satisfies ViewModule[];
