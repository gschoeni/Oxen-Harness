import { memo, useEffect, useRef, useState, type DragEvent } from "react";
import { ArrowUp, FileText, Film, Music, Paperclip, Square } from "lucide-react";
import { AttachmentImage } from "./AttachmentImage";
import { CodeReviewPicker } from "./CodeReviewPicker";
import { CompressionPicker } from "./CompressionPicker";
import { ModelPicker } from "./ModelPicker";
import { parseSlashCommand, slashSuggestions } from "./slashCommands";
import { isAudioPath, isImagePath, isVideoPath } from "../../lib/attachments";
import { advancedSettingsEnabled } from "../../lib/features";
import { useComposerSize } from "./useComposerSize";
import { isComposingKey, PROMPT_INPUT_PROPS } from "../../lib/promptInput";

/** A file staged in the prompt bar, waiting to go out with the next message. */
export interface StagedAttachment {
  path: string;
  name: string;
}

/** "1 image", "2 images · 1 video", "3 files" — what the tray holds, by kind. */
export function attachmentCountLabel(attachments: StagedAttachment[]): string {
  let images = 0;
  let videos = 0;
  let audio = 0;
  let files = 0;
  for (const a of attachments) {
    if (isImagePath(a.path)) images += 1;
    else if (isVideoPath(a.path)) videos += 1;
    else if (isAudioPath(a.path)) audio += 1;
    else files += 1;
  }
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  const parts = [
    images > 0 ? plural(images, "image", "images") : null,
    videos > 0 ? plural(videos, "video", "videos") : null,
    audio > 0 ? plural(audio, "audio track", "audio tracks") : null,
    files > 0 ? plural(files, "file", "files") : null,
  ].filter((p): p is string => p !== null);
  return parts.join(" · ");
}

export function Composer({
  busy,
  focusKey,
  onSend,
  onStop,
  onAttach,
  onDragOver,
  onDrop,
  attachments = [],
  onRemoveAttachment,
  onClearAttachments,
  placeholder,
  toolbar = true,
}: {
  busy: boolean;
  // Changes whenever a fresh/empty chat becomes active (e.g. "New chat"), so we
  // re-focus the textarea for immediate typing.
  focusKey?: string;
  onSend: (text: string) => void;
  onStop: () => void;
  /** Opens the file picker; absent where attachments don't apply (a
   *  subagent's composer), and the paperclip goes with it. */
  onAttach?: () => void;
  /** File drags over the composer itself (the textarea would otherwise treat
   *  a drop as text): forwarded to the chat's drop handling. */
  onDragOver?: (e: DragEvent<HTMLElement>) => void;
  onDrop?: (e: DragEvent<HTMLElement>) => void;
  /** Media staged for the next message — any number; the tray scrolls. */
  attachments?: StagedAttachment[];
  onRemoveAttachment?: (index: number) => void;
  onClearAttachments?: () => void;
  /** What the empty prompt says, idle and mid-run. The chat's defaults speak
   *  of the agent; a subagent's view speaks of directions and follow-ups. */
  placeholder?: { idle: string; busy: string };
  /** The model / compression / review pickers under the prompt. Off for a
   *  subagent, which runs on the model it was spawned with. */
  toolbar?: boolean;
}) {
  const [value, setValue] = useState("");
  const [slashIndex, setSlashIndex] = useState(0);
  const ref = useRef<HTMLTextAreaElement>(null);
  const suggestions = slashSuggestions(value);
  useComposerSize(ref, value);

  // Focus the composer when an empty chat is opened (new chat / initial mount).
  useEffect(() => {
    ref.current?.focus();
  }, [focusKey]);

  function submit() {
    const text = value.trim();
    if (!text) return;
    onSend(text);
    setValue("");
  }

  function chooseSlash(index: number) {
    const command = suggestions[index];
    if (!command) return;
    setValue(`${command.name} `);
    setSlashIndex(0);
    requestAnimationFrame(() => ref.current?.focus());
  }

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
      onDragOver={onDragOver}
      onDrop={(e) => {
        // Handle a path drop here and stop it: the textarea's native drop
        // would otherwise paste the drag's text into the prompt.
        onDrop?.(e);
        if (e.defaultPrevented) e.stopPropagation();
      }}
    >
      <div className="composer-inner">
        {attachments.length > 0 && (
          <div className="composer-tray" role="list" aria-label="Attached media">
            <span className="composer-tray-count">{attachmentCountLabel(attachments)}</span>
            {attachments.map((a, i) => (
              <span className="composer-tray-tile" role="listitem" key={`${a.path}-${i}`} title={a.name}>
                <TrayPreview attachment={a} />
                <span className="composer-tray-name">{a.name}</span>
                <button
                  type="button"
                  className="composer-tray-x"
                  aria-label={`Remove ${a.name}`}
                  onClick={() => onRemoveAttachment?.(i)}
                >
                  ✕
                </button>
              </span>
            ))}
            {attachments.length >= 2 && (
              <button
                type="button"
                className="composer-tray-clear"
                onClick={() => onClearAttachments?.()}
                title="Remove every attached file"
              >
                Clear
              </button>
            )}
          </div>
        )}
        <div className="composer-row">
          {onAttach && (
            <button
              type="button"
              className="attach"
              aria-label="Attach images, video, audio, or PDFs"
              title="Attach images, video, audio, or PDFs"
              onClick={onAttach}
            >
              <Paperclip size={18} />
            </button>
          )}
          <textarea
            ref={ref}
            rows={1}
            {...PROMPT_INPUT_PROPS}
            value={value}
            placeholder={
              busy
                ? (placeholder?.busy ?? "Queue a message… (sends when the agent is free)")
                : (placeholder?.idle ?? "Ask the agent to build, fix, or explain something…")
            }
            onChange={(e) => {
              setValue(e.target.value);
              setSlashIndex(0);
            }}
            onKeyDown={(e) => {
              if (isComposingKey(e.nativeEvent)) return;
              if (suggestions.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
                e.preventDefault();
                setSlashIndex((i) =>
                  e.key === "ArrowDown"
                    ? (i + 1) % suggestions.length
                    : (i - 1 + suggestions.length) % suggestions.length,
                );
                return;
              }
              if (
                suggestions.length &&
                (e.key === "Tab" || (e.key === "Enter" && !parseSlashCommand(value)))
              ) {
                e.preventDefault();
                chooseSlash(slashIndex);
                return;
              }
              if (e.key === "Escape" && suggestions.length) {
                e.preventDefault();
                setValue("");
                return;
              }
              // Backspace on an empty prompt pops the last staged file, the
              // way a chip in a token field goes.
              if (e.key === "Backspace" && value === "" && attachments.length > 0) {
                e.preventDefault();
                onRemoveAttachment?.(attachments.length - 1);
                return;
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                submit();
              }
            }}
          />
          {suggestions.length > 0 && (
            <div className="slash-menu" role="listbox" aria-label="Slash commands">
              {suggestions.map((command, index) => (
                <button
                  type="button"
                  role="option"
                  aria-selected={index === slashIndex}
                  className={index === slashIndex ? "slash-option active" : "slash-option"}
                  key={command.name}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => chooseSlash(index)}
                >
                  <code>{command.name}</code>
                  <span>{command.description}</span>
                </button>
              ))}
            </div>
          )}
          {busy ? (
            // Mid-turn the action button stops the run (killing the model stream);
            // typing + Enter still queues a message (see the placeholder).
            <button
              type="button"
              className="stop"
              aria-label="Stop generating"
              title="Stop generating"
              onClick={onStop}
            >
              <Square size={15} fill="currentColor" />
            </button>
          ) : (
            <button type="submit" className="send" aria-label="Send" disabled={!value.trim()}>
              <ArrowUp size={18} />
            </button>
          )}
        </div>
        <span className="composer-drop-hint" aria-hidden="true">
          Drop to attach
        </span>
      </div>
      {toolbar && <ComposerToolbar busy={busy} />}
    </form>
  );
}

// Draft changes don't affect the pickers; their own store subscriptions still
// update model/compression labels as sessions change. The compression picker
// is an advanced control: the mode stays switchable from Settings → Compression.
const ComposerToolbar = memo(function ComposerToolbar({ busy }: { busy: boolean }) {
  return (
    <div className="composer-toolbar">
      <ModelPicker disabled={busy} />
      {advancedSettingsEnabled() && <CompressionPicker disabled={busy} />}
      <CodeReviewPicker disabled={busy} />
    </div>
  );
});

/** A tile's picture: the image itself, or a kind icon for video/audio/files. */
function TrayPreview({ attachment }: { attachment: StagedAttachment }) {
  if (isImagePath(attachment.path)) {
    return <AttachmentImage src={attachment.path} alt={attachment.name} className="composer-tray-img" />;
  }
  const Icon = isVideoPath(attachment.path) ? Film : isAudioPath(attachment.path) ? Music : FileText;
  const kind = isVideoPath(attachment.path) ? "video" : isAudioPath(attachment.path) ? "audio" : "file";
  return (
    <span className={`composer-tray-icon ${kind}`} aria-hidden="true">
      <Icon size={20} />
    </span>
  );
}
