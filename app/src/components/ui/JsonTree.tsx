// A collapsible JSON tree (react-json-view-lite) coloured with the app's
// highlight palette, so structure reads at a glance: click a ▸ or use the
// arrow keys to open a node, and Expand all / Collapse all reset the whole
// tree. The library's own stylesheet is never loaded — every class it renders
// is ours (json-tree.css), so it follows the theme and light/dark mode.

import { useState } from "react";
import { ChevronsDownUp, ChevronsUpDown } from "lucide-react";
import { JsonView, defaultStyles, type Props as JsonViewProps } from "react-json-view-lite";
import { Button } from "./index";
import "./json-tree.css";

const STYLES: JsonViewProps["style"] = {
  ...defaultStyles,
  container: "json-tree",
  basicChildStyle: "json-tree-row",
  childFieldsContainer: "json-tree-children",
  label: "json-tree-key",
  clickableLabel: "json-tree-key clickable",
  nullValue: "json-tree-null",
  undefinedValue: "json-tree-null",
  numberValue: "json-tree-number",
  stringValue: "json-tree-string",
  booleanValue: "json-tree-boolean",
  otherValue: "json-tree-other",
  punctuation: "json-tree-punct",
  expandIcon: "json-tree-toggle expand",
  collapseIcon: "json-tree-toggle collapse",
  collapsedContent: "json-tree-collapsed",
};

type Expand = (level: number, value: unknown, field?: string) => boolean;

export function JsonTree({
  data,
  shouldExpand,
  label = "JSON",
}: {
  data: object;
  /** Which nodes start open; defaults to the first two levels. */
  shouldExpand?: Expand;
  label?: string;
}) {
  // The library reads `shouldExpandNode` once per node mount, so Expand all /
  // Collapse all remount the tree under a new key with a blanket rule.
  const [mode, setMode] = useState<{ rule: "default" | "all" | "none"; key: number }>({
    rule: "default",
    key: 0,
  });
  const rule: Expand =
    mode.rule === "all"
      ? () => true
      : mode.rule === "none"
        ? (level) => level === 0
        : (shouldExpand ?? ((level) => level < 2));
  return (
    <div className="json-tree-frame">
      <div className="json-tree-actions">
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={() => setMode((m) => ({ rule: "all", key: m.key + 1 }))}
        >
          <ChevronsUpDown size={13} aria-hidden="true" />
          Expand all
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={() => setMode((m) => ({ rule: "none", key: m.key + 1 }))}
        >
          <ChevronsDownUp size={13} aria-hidden="true" />
          Collapse all
        </Button>
      </div>
      <JsonView key={mode.key} data={data} style={STYLES} shouldExpandNode={rule} aria-label={label} />
    </div>
  );
}
