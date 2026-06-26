// Renders an image attachment by reference. The ref is either an absolute path
// (a freshly picked file in the composer) or a path relative to the session's
// workspace (how persisted attachments are stored). The backend resolves it and
// returns a data: URI, so this works for both live and resumed chats and never
// needs file:// / asset-protocol access.

import { useEffect, useState } from "react";
import { attachmentDataUri } from "../../lib/ipc";
import { useStore } from "../../lib/store";

export function AttachmentImage({
  src,
  alt,
  className,
}: {
  src: string;
  alt?: string;
  className?: string;
}) {
  const session = useStore((s) => s.session?.session_id);
  const [dataUri, setDataUri] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    setDataUri(null);
    setFailed(false);
    attachmentDataUri(src, session)
      .then((uri) => alive && setDataUri(uri))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [src, session]);

  if (failed) return null;
  if (!dataUri) return <span className={`attach-img-loading ${className ?? ""}`} aria-hidden />;
  return <img className={className} src={dataUri} alt={alt ?? "attachment"} />;
}
