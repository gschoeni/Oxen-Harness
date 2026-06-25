import { useRef, useState } from "react";
import { ArrowUp, Paperclip } from "lucide-react";

export function Composer({
  busy,
  onSend,
  onAttach,
}: {
  busy: boolean;
  onSend: (text: string) => void;
  onAttach: () => void;
}) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

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
        <button
          type="submit"
          className={`send ${busy ? "queueing" : ""}`}
          aria-label={busy ? "Add to queue" : "Send"}
          disabled={!value.trim()}
        >
          <ArrowUp size={18} />
        </button>
      </div>
    </form>
  );
}
