import { useEffect, useRef, useState } from "react";
import type { ViewProps } from "../../workbench-sdk";
import {
  viewPackagesRequest,
  viewPackageMount,
  viewPackageMove,
  viewPackageClose,
} from "../../lib/ipc";
import { registerView, removeView, views } from "./registry";
import { useOverlayOpen } from "../preview/useOverlayOpen";

export interface ViewPackage {
  digest: string;
  manifest: {
    api_version: number;
    id: string;
    title: string;
    description: string;
    entry: string;
    file_patterns: string[];
    permissions: { read: string[]; write: string[]; assets: string[]; actions: string[] };
  };
}

// Keep package matching deliberately small and predictable: '*' and '**'
// match filenames/paths; the backend remains authoritative for permission globs.
function matches(pattern: string, path: string) {
  const expression = pattern
    .split("**")
    .map((part) =>
      part
        .split("*")
        .map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
        .join("[^/]*"),
    )
    .join(".*");
  return (
    new RegExp(`^${expression}$`).test(path) ||
    (!pattern.includes("/") && new RegExp(`^${expression}$`).test(path.split("/").pop() ?? ""))
  );
}

export async function refreshPackages() {
  const packages = await viewPackagesRequest<ViewPackage[]>("list");
  for (const view of views().filter((v) => v.id.startsWith("package:"))) removeView(view.id);
  for (const pkg of packages)
    registerView({
      id: `package:${pkg.manifest.id}`,
      title: pkg.manifest.title,
      description: pkg.manifest.description,
      priority: 50,
      matches: (path) => pkg.manifest.file_patterns.some((pattern) => matches(pattern, path)),
      component: (props) => (
        <PackageSurface {...props} packageId={pkg.manifest.id} revision={pkg.digest} />
      ),
    });
  return packages;
}

export function PackageManager() {
  const [source, setSource] = useState(""),
    [review, setReview] = useState<ViewPackage>(),
    [installed, setInstalled] = useState<ViewPackage[]>([]),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  useEffect(() => {
    void refreshPackages()
      .then(setInstalled)
      .catch((e) => setError(String(e)));
  }, []);
  async function operation(fn: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="workbench-welcome">
      <h2>Make room for your own tools</h2>
      <p>
        Install a built view package from a local folder containing <code>view.json</code>. Review
        the files and actions it can access before installing.
      </p>
      <label>
        Package folder
        <input
          value={source}
          onChange={(e) => {
            setSource(e.target.value);
            setReview(undefined);
          }}
          placeholder="/path/to/my-view"
        />
      </label>
      <button
        disabled={busy || !source.trim()}
        onClick={() =>
          void operation(async () =>
            setReview(await viewPackagesRequest<ViewPackage>("inspect", { source: source.trim() })),
          )
        }
      >
        Review package
      </button>
      {review && (
        <section className="workbench-conflict">
          <h3>{review.manifest.title}</h3>
          <p>{review.manifest.description}</p>
          <dl>
            {Object.entries(review.manifest.permissions).map(([key, values]) => (
              <div key={key}>
                <dt>{key}</dt>
                <dd>{values.length ? values.join(", ") : "None"}</dd>
              </div>
            ))}
          </dl>
          <p>
            Only install code you trust with these project files. Workflow execution can incur Oxen
            charges.
          </p>
          <small>Content hash: {review.digest.slice(0, 16)}</small>
          <div>
            <button
              disabled={busy}
              onClick={() =>
                void operation(async () => {
                  await viewPackagesRequest("install", {
                    source: source.trim(),
                    digest: review.digest,
                  });
                  setInstalled(await refreshPackages());
                  setReview(undefined);
                })
              }
            >
              Grant access and install
            </button>
          </div>
        </section>
      )}
      <h3>Installed views</h3>
      {installed.length === 0 ? (
        <p>No installed packages yet. Bundled views are already available in the picker.</p>
      ) : (
        installed.map((pkg) => (
          <div className="workbench-conflict" key={pkg.manifest.id}>
            <strong>{pkg.manifest.title}</strong>
            <p>{pkg.manifest.description}</p>
            <button
              disabled={busy}
              onClick={() =>
                void operation(async () => {
                  await viewPackagesRequest("remove", { id: pkg.manifest.id });
                  setInstalled(await refreshPackages());
                })
              }
            >
              Remove and revoke access
            </button>
          </div>
        ))
      )}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}

export function PackageSurface({
  api,
  packageId,
  development = false,
  revision,
  resourcePath,
}: ViewProps & {
  packageId: string;
  development?: boolean;
  revision?: string;
  resourcePath?: string;
}) {
  const path = development ? resourcePath : api.context.target.path;
  const ref = useRef<HTMLDivElement>(null),
    label = useRef<string | undefined>(undefined);
  const [error, setError] = useState("");
  const overlay = useOverlayOpen();
  const visible = useRef(!overlay);
  visible.current = !overlay;
  useEffect(() => {
    let disposed = false,
      raf = 0,
      observer: ResizeObserver | undefined,
      instance: string | undefined;
    const bounds = () => {
      const rect = ref.current?.getBoundingClientRect();
      return {
        x: rect?.x ?? 0,
        y: rect?.y ?? 0,
        width: rect?.width ?? 1,
        height: rect?.height ?? 1,
      };
    };
    const move = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        if (instance)
          void viewPackageMove(instance, bounds(), visible.current).catch((e) => {
            if (!disposed) setError(String(e));
          });
      });
    };
    void viewPackageMount(api.context.session, packageId, path, bounds(), development)
      .then(async (id) => {
        if (disposed) {
          await viewPackageClose(id);
          return;
        }
        instance = id;
        label.current = id;
        observer = new ResizeObserver(move);
        if (ref.current) observer.observe(ref.current);
        window.addEventListener("resize", move);
        move();
      })
      .catch((e) => {
        if (!disposed) setError(String(e));
      });
    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      observer?.disconnect();
      window.removeEventListener("resize", move);
      label.current = undefined;
      if (instance)
        void viewPackageClose(instance).catch((e) =>
          api.report({ status: "error", error: String(e) }),
        );
    };
  }, [api.context.session, packageId, path, development, revision]);
  useEffect(() => {
    const rect = ref.current?.getBoundingClientRect();
    if (label.current && rect)
      void viewPackageMove(
        label.current,
        { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
        !overlay,
      ).catch((e) => setError(String(e)));
  }, [overlay]);
  return (
    <div ref={ref} style={{ height: "100%", position: "relative" }}>
      {error && (
        <div className="workbench-error" role="alert">
          {error}
        </div>
      )}
    </div>
  );
}
