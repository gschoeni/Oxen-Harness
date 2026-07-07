import { Markdown } from "../../components/ui/Markdown";
import { ToolCall } from "./ToolCall";
import { ApiKeyPrompt } from "./ApiKeyPrompt";
import { RetryPrompt } from "./RetryPrompt";
import { AttachmentImage } from "./AttachmentImage";
import type { Item } from "./thread";

/** Render one thread item: a user bubble, an assistant message (Markdown, or a
 *  "thinking" indicator while empty), or a tool-call card. `now` is passed in so
 *  running cards re-time as the parent ticks. */
export function ThreadItem({ item, now }: { item: Item; now: number }) {
  if (item.kind === "user") {
    return (
      <div className="msg user">
        {item.images && item.images.length > 0 && (
          <div className="msg-attachments">
            {item.images.map((src, i) => (
              <AttachmentImage key={`${src}-${i}`} src={src} className="msg-attachment-img" />
            ))}
          </div>
        )}
        {item.text && <div className="msg-user-text">{item.text}</div>}
      </div>
    );
  }

  if (item.kind === "notice") {
    return <div className="msg notice">{item.text}</div>;
  }

  if (item.kind === "apikey") {
    return <ApiKeyPrompt item={item} />;
  }

  if (item.kind === "retry") {
    return <RetryPrompt item={item} />;
  }

  if (item.kind === "assistant") {
    return (
      <div className={`msg assistant ${item.error ? "error" : ""}`}>
        <div className="role">Oxen</div>
        {item.text ? (
          item.error ? <div className="body">{item.text}</div> : <Markdown text={item.text} />
        ) : null}
        {/* Keep an activity indicator visible the whole time the bubble is
            streaming — including the silent stretch while the model writes a
            tool call's arguments (a canvas document, clarifying questions) after
            a short preamble, when the bubble already has text. */}
        {item.streaming && (
          <span className={`thinking ${item.text ? "trailing" : ""}`}>
            <span />
            <span />
            <span />
          </span>
        )}
      </div>
    );
  }

  return <ToolCall item={item} now={now} />;
}
