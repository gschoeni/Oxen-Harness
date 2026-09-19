import type { ViewModule } from "../../workbench-sdk";
import { Blocks } from "lucide-react";
import { PackageManager } from "../../features/workbench/packages";
export default [
  {
    id: "view-manager",
    agentVisible: false,
    title: "Manage views",
    icon: Blocks,
    description: "Install your own view packages.",
    component: PackageManager,
  },
] satisfies ViewModule[];
