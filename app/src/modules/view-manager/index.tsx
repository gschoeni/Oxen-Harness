import type { ViewModule } from "../../workbench-sdk";
import { PackageManager } from "../../features/workbench/packages";
export default [
  {
    id: "view-manager",
    title: "Manage views",
    description: "Install your own view packages.",
    component: PackageManager,
  },
] satisfies ViewModule[];
