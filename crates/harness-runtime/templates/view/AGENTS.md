# Build this view in place

This is an Oxen work view. The user is building it with you inside the app.
Edit HTML/CSS/JavaScript here. No app build, server, or package manager is needed.
Use `develop_view` to check, preview, test, inspect status, and install. Checks
return a content digest; preview/install require that exact digest. Source changes
refresh an active preview. Repair errors using its report_path and runtime log.
Keep this project's data in its declared permission paths; view.json is the
capability contract. A permission change pauses refresh until preview is explicitly
restarted. Do not broaden access without the user's requested functionality.

Use window.oxenView; see oxen-view.d.ts for types. The host handles file reads,
revision-checked saves, scoped assets, navigation and chat context. Retain drafts
with retain/restore before preview refresh; report({dirty:true}) blocks refresh
if a draft is not safely retained. Surface conflicts and preserve the user's input.
Theme CSS uses --bg, --surface, --text, --text-secondary, --accent, and --border.
Keep controls labelled, keyboard accessible, and useful at narrow panel widths.

Register browser tests with api.test(name, async ({assert})=>...). Test in the
actual preview before installing. A passing structural check alone is not a
passing browser test. Tests may mutate only their own fixture files, and should
not trigger paid workflows. Runtime reports belong to the tested revision.

For React or another bundler, bundle local assets into a separate output folder
containing view.json. Use its build/watch command and preview that output folder.
Do not point the preview at node_modules. No remote scripts, eval, general native
commands or raw network access are available. The package is the extension boundary;
new native capabilities require a host change, ordinary custom UI does not.
