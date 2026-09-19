import { lazy } from "react";
import { Braces } from "lucide-react";
import type { ViewModule } from "../../workbench-sdk";
const ViewStudio = lazy(() =>
  import("./ViewStudio").then((module) => ({ default: module.ViewStudio })),
);
export default [
  {
    id: "view-studio",
    title: "View Studio",
    icon: Braces,
    description: "Build your own views with your agent. Preview, test, and install live.",
    component: ViewStudio,
  },
] satisfies ViewModule[];
