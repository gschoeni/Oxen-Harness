import { memo } from "react";
import { Markdown } from "../../components/ui/Markdown";
import { useThrottled } from "../../lib/useThrottled";
import { ThinkingIndicator } from "./ThinkingIndicator";
import { ToolCall } from "./ToolCall";
import { ApiKeyPrompt } from "./ApiKeyPrompt";
import { RetryPrompt } from "./RetryPrompt";
import { AttachmentImage } from "./AttachmentImage";
import type { Item } from "./thread";

/** Render one thread item: a user bubble, an assistant message (Markdown, or a
 *  "thinking" indicator while empty), or a tool-call card. Memoized — items are
 *  immutable snapshots, so during streaming only the item actually receiving
 *  content re-renders (a long thread re-parsing every Markdown bubble per
 *  update is real jank). Anything time-driven (spinners, elapsed labels) ticks
 *  inside the leaf components. */
export const ThreadItem = memo(function ThreadItem({ item }: { item: Item }) {
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
    return <AssistantBubble item={item} />;
  }

  return <ToolCall item={item} />;
});

/** How often a streaming reply re-parses its Markdown. Tokens land in the
 *  store every ~50 ms; parsing a long reply that often is the single biggest
 *  cost of a fast stream, and nobody reads faster than ten repaints a second. */
const STREAM_MARKDOWN_MS = 100;

function AssistantBubble({ item }: { item: Extract<Item, { kind: "assistant" }> }) {
  // Throttle only while streaming: the moment the bubble settles, the final
  // text must render at once (not up to 100 ms later), so the settled item
  // bypasses the throttled value entirely.
  const throttled = useThrottled(item.text, STREAM_MARKDOWN_MS);
  const text = item.streaming ? throttled : item.text;
  return (
    <div className={`msg assistant ${item.error ? "error" : ""}`}>
      <div className="role">Oxen</div>
      {text ? (
        item.error ? <div className="body">{text}</div> : <Markdown text={text} />
      ) : null}
      {/* Keep an activity indicator visible the whole time the bubble is
          streaming — including the silent stretch while the model writes a
          tool call's arguments (a canvas document, clarifying questions) after
          a short preamble, when the bubble already has text. The indicator
          reads the live text, not the throttled one, so it flips to "writing"
          on the first token. */}
      {item.streaming && (
        <ThinkingIndicator writing={!!item.text} trailing={!!item.text} />
      )}
    </div>
  );
}
