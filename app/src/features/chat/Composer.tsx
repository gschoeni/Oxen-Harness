import { useEffect, useRef, useState } from "react";
import { ArrowUp, Paperclip, Square } from "lucide-react";
import { CodeReviewPicker } from "./CodeReviewPicker";
import { CompressionPicker } from "./CompressionPicker";
import { ModelPicker } from "./ModelPicker";
import { parseSlashCommand, slashSuggestions } from "./slashCommands";

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
  const [slashIndex, setSlashIndex] = useState(0);
  const ref = useRef<HTMLTextAreaElement>(null);
  const suggestions = slashSuggestions(value);

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
            setSlashIndex(0);
            resize();
          }}
          onKeyDown={(e) => {
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
      <div className="composer-toolbar">
        <ModelPicker disabled={busy} />
        <CompressionPicker disabled={busy} />
        <CodeReviewPicker disabled={busy} />
      </div>
    </form>
  );
}
