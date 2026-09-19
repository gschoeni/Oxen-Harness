# Work views

The tabs above the conversation own both the agent and its work view. The right
column has a view picker and back/forward history. It has no separate tab strip.
Pin a view to keep agent events from changing what you are looking at.

Choose **Oxen workflow** in the picker to create an image studio, image-to-video
pipeline, or empty graph. Click **Node** to add a node, or drag one from the
palette. Drag between matching handles to connect nodes; select a node to edit
its settings. Save and Run are separate actions. **Build with agent** saves your
draft and adds the workflow file to the conversation's composer.

## Files and execution

Graphs are ordinary `*.graph.json` files. The agent can discover the schema with
`list_views`, edit files with its normal filesystem tools, and call `open_view`
or `open_file` to display a graph. `inspect_view` reports the disk revision,
validation errors, and the renderer's reported selection/dirty status. A renderer
that is not mounted is reported as unavailable or unmounted, not inferred from
the contents of its file.

The nine node kinds are `prompt`, `rewrite`, `image_input`, `video_input`, `image`,
`video`, `upscale`, `video_upscale`, and `output`. Media models and their parameter
schemas come from Oxen's catalog. Image/video nodes use the existing queue,
reference uploader, media library, and spend-approval flow. Rewrite nodes use
Oxen chat completions with a bounded response and attribute usage to the session.

```json
{
  "version": 1,
  "title": "A first image",
  "nodes": [
    { "id": "prompt", "kind": "prompt", "position": { "x": 0, "y": 0 },
      "config": { "text": "A red fox in a snowy forest" } },
    { "id": "image", "kind": "image", "position": { "x": 300, "y": 0 },
      "config": {} },
    { "id": "output", "kind": "output", "position": { "x": 300, "y": 250 },
      "config": {} }
  ],
  "edges": [
    { "id": "prompt-image", "source": "prompt", "target": "image", "target_port": "prompt" },
    { "id": "image-output", "source": "image", "target": "output", "target_port": "input" }
  ]
}
```

An omitted image/video model uses the project default. Upscalers default to
`flux-image-upscaler` and `flux-video-upscaler`. Model-specific parameters live
under `config.params`; reference files belong in input nodes and connections.
Unknown fields are preserved while editing. Unknown node kinds remain visible
but cannot run. The limits are 128 nodes, 512 edges, and a 4 MiB editable document.

Execution is explicit, through Run or `run_workflow`. Every run captures an
immutable graph snapshot and revision. Records and per-node outputs are written
under `.oxen-harness/workflow-runs/`; downloaded media stays in the existing
generation library. Editing, opening, and restoring a graph never run it.
Budget prompts apply to each media generation. There is no aggregate graph price
guarantee: prices depend on the chosen models and their parameters.

Stop lets the current generation finish and preserves its result, then skips
downstream nodes. The gallery can cancel an individual queued media job. After a
host restart, unfinished runs are shown as interrupted; inspect the media library
before starting another run because the provider may already have billed jobs.
Runs are never automatically retried or resumed.

## Bundled modules

Add `app/src/modules/<your-view>/index.tsx`, exporting a default `ViewModule[]`.
The module loader discovers it automatically. The application store, layout,
and DockColumn do not need another view-specific branch.

```tsx
import { useDocument, type ViewModule, type ViewProps } from "../../workbench-sdk";

function Notes({ api }: ViewProps) {
  const path = api.context.target.path;
  return path ? <Note api={api} path={path} /> : <p>Open a note from the file tree.</p>;
}

function Note({ api, path }: ViewProps & { path: string }) {
  const document = useDocument(api, path);
  return <>
    <textarea value={document.content} onChange={e => document.edit(e.target.value)} />
    <button onClick={() => document.save().catch(console.error)}>Save</button>
    {document.error && <p role="alert">{document.error}</p>}
  </>;
}

export default [{
  id: "notes", title: "Notes", description: "Project notes",
  matches: path => path.endsWith(".notes.json"), filePatterns: ["*.notes.json"],
  requiresFile: true, documentSchema: { type: "object" },
  priority: 20, component: Notes,
}] satisfies ViewModule[];
```

The host receives each bundled view's descriptor when the desktop opens a
conversation. `list_views` exposes its file patterns and optional document schema,
and `open_view` can select it by id or matching file. `agentVisible: false` hides
UI-only utilities. Installed packages are discovered directly from their manifests.

Production modules should also render `document.conflict` and offer an explicit
choice of the on-disk version or retained draft. See the workflow module for a
complete example. The shared document store retains drafts across view/context
switches; the backend rejects a save against an old revision and keeps a previous
saved copy in `.oxen-harness/recovery/`. Save drafts before quitting the app.

Modules use `WorkbenchAPI` for project documents, scoped assets, host actions,
file-change subscriptions, navigation, and adding context to the composer. Avoid
imports from `lib/store` or `lib/ipc` in new modules. The legacy built-in adapters
are migration seams; the graph module only imports the SDK and its own code.

## Installable view packages

Choose **Manage views**, enter a built package's directory, and review it. The
approval lists its read/write/asset path patterns and action capabilities.
Installation checks the approved content hash and copies the assets into an
immutable version under `~/.oxen-harness/views/`. It runs no package manager or
install script. An update needs a new review. Removal revokes access and leaves
project documents intact.

A package contains `view.json`, its HTML entry, and local JS/CSS/assets. API v1
manifests declare `api_version`, `id`, `title`, `description`, `entry`, optional
`file_patterns`, and `permissions` (`read`, `write`, `assets`, `actions`). Paths
are workspace-relative glob patterns. The only paid action grant is
`workflow.run`. Limit packages to 512 assets and 20 MiB; symlinks are rejected.

The native host injects `window.oxenView`, with the same document/context/action
contract as the bundled SDK. A package may use any framework or plain JavaScript.
It runs in a separate native webview, with no general Tauri commands or core
permissions. The bridge binds calls to that view's session and approved paths;
the package cannot supply a different workspace or session. Remote scripts,
frames, and network access are blocked by its content policy. Media assets are
served through a scoped protocol; the per-asset limit is 100 MiB. Theme colors are
injected as CSS custom properties.

Two examples ship in `examples/views/`:

- `notes/` is ready to install, needs no build, and requests only `notes/*.json`.
- `workflow/` builds the **same** graph component as an independent package:
  run `cd app && pnpm build:workflow-view`, then install
  `examples/views/workflow/dist`. It requests workflow documents, references in
  `assets/`, outputs in `generations/`, and explicit workflow execution. Adjust
  and re-review its manifest if your project uses different directories.

Package rendering is desktop-native. The shared document and workflow service
is also available over HTTP:
`POST /v1/sessions/{id}/workbench/{action}` with a JSON payload and the server's
normal authentication. Actions include `read`, `save`, `list`, `open`, `inspect`,
`report`, `register_views` (bundled descriptor discovery), `models`, `run`, `status`, `latest`, `cancel`, and `add_context`.
`view.open` and `view.context` events carry the owning session.
