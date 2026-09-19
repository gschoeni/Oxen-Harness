// A bundled module exports ViewModule[] from its own index file. Adding a
// directory is sufficient; the host and application store stay unchanged.
import { registerView } from "../features/workbench/registry";
import type { ViewModule } from "../workbench-sdk";
const bundles = import.meta.glob<{ default: ViewModule[] }>("./*/index.tsx", { eager: true });
for (const bundle of Object.values(bundles)) for (const view of bundle.default) registerView(view);
