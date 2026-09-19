// Build the bundled graph as an independent installable package. The app
// executable is untouched; install the resulting dist/ through Manage views.
import { build } from "vite";
import react from "@vitejs/plugin-react";
import { copyFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
const root = fileURLToPath(new URL("../../examples/views/workflow", import.meta.url));
await build({
  configFile: false,
  root,
  base: "./",
  publicDir: false,
  plugins: [react()],
  resolve: {
    dedupe: ["react", "react-dom"],
    alias: {
      react: fileURLToPath(new URL("../node_modules/react", import.meta.url)),
      "react-dom": fileURLToPath(new URL("../node_modules/react-dom", import.meta.url)),
    },
  },
  build: { outDir: resolve(root, "dist"), emptyOutDir: true },
});
await copyFile(resolve(root, "view.json"), resolve(root, "dist/view.json"));
