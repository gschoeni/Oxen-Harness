import React from "react";
import { createRoot } from "react-dom/client";
import { WorkflowView } from "../../../app/src/modules/oxen-workflow/WorkflowView";
import type { WorkbenchAPI } from "../../../app/src/workbench-sdk";
import "../../../app/src/styles/tokens.css";
import "../../../app/src/features/workbench/workbench.css";

const root = document.getElementById("root");
const api = (window as unknown as { oxenView: WorkbenchAPI }).oxenView;
if (root && api) createRoot(root).render(<WorkflowView api={api} />);
