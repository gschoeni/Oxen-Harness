import { useEffect, useRef, useState } from "react";
import { ArrowUp, Paperclip, Square } from "lucide-react";
import { CodeReviewPicker } from "./CodeReviewPicker";
import { CompressionPicker } from "./CompressionPicker";
import { ModelPicker } from "./ModelPicker";

export function Composer({
  busy,
  focusKey,
  onSend,
  onStop,
  onAttach,
}: {
  busy: boolean;
  // Changes whenever a fresh/empty chat becomes active (e.g. "New chat"), so we
  // re-focus the textarea for immediate typing.
  focusKey?: string;
  onSend: (text: string) => void;
  onStop: () => void;
  onAttach: () => void;
}) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

  // Focus the composer when an empty chat is opened (new chat / initial mount).
  useEffect(() => {
    ref.current?.focus();
  }, [focusKey]);

  function submit() {
    const text = value.trim();
    if (!text) return;
    onSend(text);
    setValue("");
    if (ref.current) ref.current.style.height = "auto";
  }

  function resize() {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
  }

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="composer-inner">
        <button
          type="button"
          className="attach"
          aria-label="Attach images or PDFs"
          title="Attach images or PDFs"
          onClick={onAttach}
        >
          <Paperclip size={18} />
        </button>
        <textarea
          ref={ref}
          rows={1}
          value={value}
          placeholder={
            busy
              ? "Queue a message… (sends when the agent is free)"
              : "Ask the agent to build, fix, or explain something…"
          }
          onChange={(e) => {
            setValue(e.target.value);
            resize();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
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
      <div className="composer-toolbar">
        <ModelPicker disabled={busy} />
        <CompressionPicker disabled={busy} />
        <CodeReviewPicker disabled={busy} />
      </div>
    </form>
  );
}
