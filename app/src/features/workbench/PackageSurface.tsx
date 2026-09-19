import { useEffect, useRef, useState } from "react";
import type { ViewProps } from "../../workbench-sdk";
import {
  viewPackageMount,
  viewPackageMove,
  viewPackageClose,
} from "../../lib/ipc";
import { useOverlayOpen } from "../preview/useOverlayOpen";

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
    setError("");
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
          void viewPackageMove(instance, bounds(), visible.current).catch(
            (e) => {
              if (!disposed) setError(String(e));
            },
          );
      });
    };
    void viewPackageMount(
      api.context.session,
      packageId,
      path,
      bounds(),
      development,
    )
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
