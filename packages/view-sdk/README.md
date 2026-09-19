# @oxen/view-sdk

The portable, framework-neutral contract for Oxen work views. The desktop host
injects `window.oxenView` before a package loads. Plain JavaScript needs no build;
TypeScript/React packages can import `getViewAPI` and these types. This directory
is a local npm package (`npm install /path/to/OxenHarness/packages/view-sdk`), not
an assertion that a version has been published to a registry.

Use View Studio to scaffold, live preview, inspect runtime errors, run registered
browser tests, and install an immutable version. Code changes refresh the preview;
changed permissions require an explicit preview restart. Saving app code never
rebuilds the desktop host. See `app/WORKBENCH.md` for the complete contract.

`api.test('name', async ({assert}) => { ... })` runs only when requested, in the
real preview with its normal permissions. Tests should create their own fixture
files and avoid destructive actions. `retain`/`restore` keep JSON drafts across
reloads. Ordinary document writes remain revision-checked. Call `report({dirty:
true})` to defer automatic reload until a draft is saved or safely retained.
