// One generation, full frame: the picture or clip at the top, then either
// the designed read of everything that made it (prompt, request, output,
// lineage, chat) or the manifest row itself as JSON. The lineage trail is
// the point of the view — every row is a real link in the chain the file
// came from (an attachment, an earlier generation, a project file) or went
// into (later generations that used this one), and the ones that are
// library items open in place.

import { useMemo, useState, type DragEvent, type ReactNode } from "react";
import { Button } from "../../components/ui";
import { Check, Copy, FileCode2, FolderTree, MessageSquare, Paperclip } from "lucide-react";
import { useStore } from "../../lib/store";
import type { MediaItem, MediaSource } from "../../lib/types";
import { elapsedSince, fmtBytes, fmtUsd, whenLabel } from "../../lib/media";
import { useAssetSrc } from "../files/useAssetSrc";
import { setDragPaths } from "../files/dnd";

const NONE: MediaItem[] = [];
type View = "details" | "raw";

export function GenerationDetail({
  item,
  workspace,
  bust,
  onOpen,
}: {
  item: MediaItem;
  workspace: string;
  bust: number;
  /** Show another generation of the same library (a lineage row). */
  onOpen: (id: string) => void;
}) {
  const [view, setView] = useState<View>("details");
  const stageAttachment = useStore((s) => s.stageAttachment);
  const openInViewer = useStore((s) => s.openInViewer);
  const revealInFiles = useStore((s) => s.revealInFiles);
  const resume = useStore((s) => s.resume);
  const sessionId = useStore((s) => s.session?.session_id);
  const fromThisChat = item.session === sessionId;
  return (
    <div className="gallery-detail" aria-label="Generation details">
      <Preview item={item} workspace={workspace} bust={bust} />
      <div className="gallery-detail-body">
        <div className="segmented gallery-detail-mode" role="tablist" aria-label="Metadata view">
          {(["details", "raw"] as View[]).map((v) => (
            <button
              key={v}
              type="button"
              role="tab"
              aria-selected={view === v}
              className={view === v ? "active" : ""}
              onClick={() => setView(v)}
            >
              {v === "details" ? "Details" : "Raw"}
            </button>
          ))}
        </div>
        {view === "details" ? <Details item={item} workspace={workspace} onOpen={onOpen} /> : <Raw item={item} />}
        <div className="gallery-actions">
          {item.path && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => stageAttachment(`${workspace}/${item.path}`)}
              title="Attach to your next message as a reference"
            >
              <Paperclip size={13} /> Use as reference
            </Button>
          )}
          {item.path && (
            <Button type="button" size="sm" variant="outline" onClick={() => openInViewer([item.path!])}>
              <FileCode2 size={13} /> Open in editor
            </Button>
          )}
          {item.path && (
            <Button type="button" size="sm" variant="outline" onClick={() => revealInFiles(item.path!)}>
              <FolderTree size={13} /> Reveal in Files
            </Button>
          )}
          <CopyButton text={item.agent_prompt ?? item.prompt} label="Copy prompt" />
          {!fromThisChat && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => void resume(item.session)}
              title="Open the chat that asked for this"
            >
              <MessageSquare size={13} /> Open chat
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

/** The whole frame, contained: tiles crop to squares, so this is where the
 *  real aspect ratio shows. Draggable into the chat. */
function Preview({ item, workspace, bust }: { item: MediaItem; workspace: string; bust: number }) {
  const src = useAssetSrc(workspace, item.path ?? "", bust);
  const poster = useAssetSrc(workspace, item.poster ?? "", bust);
  if (!item.path || !src) {
    return (
      <div className="gallery-detail-preview">
        <div className="gallery-detail-nofile">{item.error ?? item.status.replace("_", " ")}</div>
      </div>
    );
  }
  const drag = (e: DragEvent) => setDragPaths(e.dataTransfer, [`${workspace}/${item.path}`]);
  return (
    <div className="gallery-detail-preview">
      {item.kind === "video" ? (
        <video
          src={src}
          controls
          autoPlay
          muted
          loop
          playsInline
          preload="metadata"
          poster={item.poster && poster ? poster : undefined}
          draggable
          onDragStart={drag}
          title="Drag into the chat to attach"
        />
      ) : (
        <img src={src} alt={item.prompt} draggable onDragStart={drag} title="Drag into the chat to attach" />
      )}
    </div>
  );
}

function Details({ item, workspace, onOpen }: { item: MediaItem; workspace: string; onOpen: (id: string) => void }) {
  const sessionId = useStore((s) => s.session?.session_id);
  const library = useStore((s) => s.media[workspace]) ?? NONE;
  const params = Object.entries(item.params ?? {}).filter(([, v]) => v !== null && v !== undefined && v !== "");
  const took = item.completed_at ? elapsedSince(item.created_at, item.completed_at * 1000) : null;
  const ext = item.path?.match(/\.([a-z0-9]+)$/i)?.[1]?.toUpperCase();
  const output = [
    item.width && item.height ? `${item.width}×${item.height}` : null,
    item.duration_secs ? `${item.duration_secs}s` : null,
    item.bytes ? fmtBytes(item.bytes) : null,
    ext,
  ].filter(Boolean);
  return (
    <>
      <section className="gallery-section">
        <p className="gallery-detail-prompt">{item.agent_prompt ?? item.prompt}</p>
        {item.agent_prompt && (
          <p className="gallery-detail-sent" title="The prompt after reference labels were rewritten for the model">
            Sent as: {item.prompt}
          </p>
        )}
      </section>

      <section className="gallery-section">
        <dl className="gallery-facts">
          <Fact label="Model">
            <span className="gallery-mono" title={item.model}>
              {item.model}
            </span>
            <span className="gallery-facts-aside">{item.kind}</span>
          </Fact>
          <Fact label="Status">
            <span className={`gallery-status ${item.status}`}>{item.status.replace("_", " ")}</span>
            {item.error && <span className="gallery-facts-error">{item.error}</span>}
          </Fact>
          {output.length > 0 && <Fact label="Output">{output.join(" · ")}</Fact>}
          <Fact label="When">
            {whenLabel(item.created_at)}
            {took && <span className="gallery-facts-aside">took {took}</span>}
          </Fact>
          {item.cost_usd !== null && <Fact label="Cost">{fmtUsd(item.cost_usd)}</Fact>}
          {item.seed !== null && item.seed !== undefined && (
            <Fact label="Seed">
              <span className="gallery-mono">{String(item.seed)}</span>
            </Fact>
          )}
        </dl>
      </section>

      {params.length > 0 && (
        <section className="gallery-section">
          <h4 className="gallery-section-label">Request</h4>
          <dl className="gallery-facts params">
            {params.map(([k, v]) => (
              <Fact key={k} label={k} mono>
                <span className="gallery-mono">{typeof v === "string" ? v : JSON.stringify(v)}</span>
              </Fact>
            ))}
          </dl>
        </section>
      )}

      <Lineage item={item} library={library} workspace={workspace} onOpen={onOpen} />

      <section className="gallery-section">
        <h4 className="gallery-section-label">Identity</h4>
        <dl className="gallery-facts">
          <Fact label="Chat">
            <span className="gallery-mono" title={item.session}>
              {item.session === sessionId ? "this chat" : shortId(item.session)}
            </span>
          </Fact>
          <Fact label="Generation">
            <span className="gallery-mono" title={item.id}>
              {item.id}
            </span>
          </Fact>
          {item.batch && (
            <Fact label="Batch">
              <span className="gallery-mono" title={item.batch}>
                {shortId(item.batch)}
              </span>
              <span className="gallery-facts-aside">#{item.index}</span>
            </Fact>
          )}
          {item.path && (
            <Fact label="File">
              <span className="gallery-mono gallery-path" title={item.path}>
                {item.path}
              </span>
            </Fact>
          )}
        </dl>
      </section>
    </>
  );
}

function Fact({ label, mono = false, children }: { label: string; mono?: boolean; children: ReactNode }) {
  return (
    <div className="gallery-fact">
      <dt className={mono ? "gallery-mono" : ""}>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/** A trail: what fed this generation above it, the generation itself, and
 *  what it fed below. Rows that are library items open in place. */
function Lineage({
  item,
  library,
  workspace,
  onOpen,
}: {
  item: MediaItem;
  library: MediaItem[];
  workspace: string;
  onOpen: (id: string) => void;
}) {
  const inputs = useMemo(() => lineageInputs(item), [item]);
  const parent = item.parent ? library.find((m) => m.path === item.parent) : undefined;
  const children = useMemo(() => derivedFrom(item, library), [item, library]);
  const empty = inputs.length === 0 && !item.parent && children.length === 0;
  return (
    <section className="gallery-section">
      <h4 className="gallery-section-label">Lineage</h4>
      {empty ? (
        <p className="gallery-lineage-none">From the prompt alone. Nothing has been made from it yet.</p>
      ) : (
        <ol className="gallery-trail">
          {inputs.map((s) => (
            <SourceRow key={s.path} source={s} workspace={workspace} library={library} onOpen={onOpen} />
          ))}
          {item.parent && (
            <TrailRow
              kind="in"
              path={parent?.path ?? item.parent}
              workspace={workspace}
              title={fileName(item.parent)}
              detail={parent ? `earlier generation · ${verb(parent, item)}` : "earlier file it varies"}
              onClick={parent ? () => onOpen(parent.id) : undefined}
            />
          )}
          <TrailRow
            kind="self"
            path={item.path}
            workspace={workspace}
            title={item.path ? fileName(item.path) : "this generation"}
            detail={`this ${item.kind} · ${item.model}`}
          />
          {children.map((c) => (
            <TrailRow
              key={c.id}
              kind="out"
              path={c.path ?? c.poster}
              workspace={workspace}
              title={c.path ? fileName(c.path) : c.status.replace("_", " ")}
              detail={`${c.kind} made from this · ${whenLabel(c.created_at)}`}
              onClick={() => onOpen(c.id)}
            />
          ))}
        </ol>
      )}
    </section>
  );
}

function SourceRow({
  source,
  workspace,
  library,
  onOpen,
}: {
  source: MediaSource;
  workspace: string;
  library: MediaItem[];
  onOpen: (id: string) => void;
}) {
  const origin = source.generation ? library.find((m) => m.id === source.generation) : undefined;
  const name = source.label ? `${source.label} ${fileName(source.source)}` : fileName(source.source);
  const detail =
    source.origin === "generation"
      ? `earlier generation${origin ? ` · ${origin.model}` : ""}`
      : source.origin === "attachment"
        ? `attached to the chat · ${source.kind}`
        : source.kind === LEGACY_KIND
          ? "reference"
          : `project file · ${source.kind}`;
  return (
    <TrailRow
      kind="in"
      path={source.path}
      workspace={workspace}
      title={name}
      detail={detail}
      hint={source.source}
      onClick={origin ? () => onOpen(origin.id) : undefined}
    />
  );
}

function TrailRow({
  kind,
  path,
  workspace,
  title,
  detail,
  hint,
  onClick,
}: {
  kind: "in" | "self" | "out";
  path: string | null | undefined;
  workspace: string;
  title: string;
  detail: string;
  hint?: string;
  onClick?: () => void;
}) {
  const body = (
    <>
      <TrailThumb path={path} workspace={workspace} />
      <span className="gallery-trail-text">
        <span className="gallery-trail-title">{title}</span>
        <span className="gallery-trail-detail">{detail}</span>
      </span>
    </>
  );
  return (
    <li className={`gallery-trail-row ${kind}`} title={hint ?? title}>
      {onClick ? (
        <button type="button" className="gallery-trail-link" onClick={onClick}>
          {body}
        </button>
      ) : (
        <span className="gallery-trail-link static">{body}</span>
      )}
    </li>
  );
}

function TrailThumb({ path, workspace }: { path: string | null | undefined; workspace: string }) {
  const src = useAssetSrc(workspace, path ?? "", 0);
  const picture = !!path && /\.(png|jpe?g|webp|gif|avif)$/i.test(path);
  if (!path || !src || !picture) return <span className="gallery-trail-thumb blank" aria-hidden="true" />;
  return (
    <img
      className="gallery-trail-thumb"
      src={src}
      alt=""
      draggable
      onDragStart={(e: DragEvent) => setDragPaths(e.dataTransfer, [`${workspace}/${path}`])}
    />
  );
}

function Raw({ item }: { item: MediaItem }) {
  const text = useMemo(() => JSON.stringify(item, null, 2), [item]);
  return (
    <section className="gallery-section gallery-raw">
      <div className="gallery-raw-bar">
        <span className="gallery-section-label">Manifest row</span>
        <CopyButton text={text} label="Copy JSON" />
      </div>
      <pre className="gallery-raw-json">{text}</pre>
    </section>
  );
}

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <Button type="button" size="sm" variant="outline" onClick={copy}>
      {copied ? <Check size={13} /> : <Copy size={13} />} {copied ? "Copied" : label}
    </Button>
  );
}

/** Rows recorded before provenance was tracked only know the stored copy;
 *  this marks the bare row synthesized for each one. */
const LEGACY_KIND = "reference";

/** The references as provenance rows. */
export function lineageInputs(item: MediaItem): MediaSource[] {
  if (item.sources.length > 0) return item.sources;
  return item.refs.map((path) => ({
    path,
    origin: "file",
    label: null,
    source: path,
    generation: null,
    kind: LEGACY_KIND,
    sha256: "",
  }));
}

/** Later generations that used this one: as a traced reference, or as the
 *  declared parent. */
export function derivedFrom(item: MediaItem, library: MediaItem[]): MediaItem[] {
  return library.filter(
    (m) =>
      m.id !== item.id &&
      ((item.path !== null && m.parent === item.path) ||
        m.sources.some((s) => s.generation === item.id || (item.path !== null && s.source === item.path))),
  );
}

/** What the agent said this item does to its parent is not recorded, so
 *  only the one relation the kinds prove gets a name. */
function verb(parent: MediaItem, child: MediaItem): string {
  return parent.kind === "image" && child.kind === "video" ? "animated" : "varied";
}

const fileName = (path: string) => path.split("/").pop() ?? path;
const shortId = (id: string) => (id.length > 12 ? `${id.slice(0, 8)}…` : id);
