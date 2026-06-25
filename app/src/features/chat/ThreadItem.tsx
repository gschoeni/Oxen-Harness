import { Markdown } from "../../components/ui/Markdown";
import { ToolCall } from "./ToolCall";
import type { Item } from "./thread";

/** Render one thread item: a user bubble, an assistant message (Markdown, or a
 *  "thinking" indicator while empty), or a tool-call card. `now` is passed in so
 *  running cards re-time as the parent ticks. */
export function ThreadItem({ item, now }: { item: Item; now: number }) {
  if (item.kind === "user") {
    return <div className="msg user">{item.text}</div>;
  }

  if (item.kind === "assistant") {
    return (
      <div className={`msg assistant ${item.error ? "error" : ""}`}>
        <div className="role">Oxen</div>
        {item.text ? (
          item.error ? <div className="body">{item.text}</div> : <Markdown text={item.text} />
        ) : (
          <span className="thinking">
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
