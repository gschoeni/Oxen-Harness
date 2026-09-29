// The CodeMirror 6 wrapper: syntax highlighting (language picked from the
// filename, grammar lazy-loaded), autocomplete (language completions where the
// grammar provides them, buffer-word completion everywhere), and the standard
// editing chrome (line numbers, search, bracket matching) via basicSetup.
// Colors come from CSS variables (defined in files.css) so the editor follows
// the app's theme and light/dark mode for free.

import { useEffect, useRef, useState } from "react";
import { basicSetup, EditorView } from "codemirror";
import { Annotation, Compartment, EditorState } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { indentWithTab } from "@codemirror/commands";
import { completeAnyWord } from "@codemirror/autocomplete";
import { HighlightStyle, LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { tags } from "@lezer/highlight";

/** A selected range, ready to stage as chat context. */
export interface EditorSelection {
  code: string;
  /** 1-based first/last line of the selection. */
  start: number;
  end: number;
}

const theme = EditorView.theme({
  "&": { height: "100%", fontSize: "12.5px", backgroundColor: "transparent", color: "var(--text)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace)",
    lineHeight: "1.55",
  },
  ".cm-content": { caretColor: "var(--accent)", padding: "10px 0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground":
    { background: "var(--cm-selection)" },
  ".cm-gutters": {
    background: "transparent",
    color: "var(--text-tertiary)",
    border: "none",
    paddingLeft: "4px",
  },
  ".cm-activeLine": { background: "var(--cm-activeline)" },
  ".cm-activeLineGutter": { background: "transparent", color: "var(--text)" },
  ".cm-selectionMatch, .cm-searchMatch": { background: "var(--cm-selection)" },
  ".cm-tooltip": {
    background: "var(--surface)",
    color: "var(--text)",
    border: "1px solid var(--separator)",
    borderRadius: "6px",
    overflow: "hidden",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
    background: "var(--accent)",
    color: "var(--bg)",
  },
  ".cm-panels": {
    background: "var(--surface)",
    color: "var(--text)",
    borderColor: "var(--separator)",
  },
});

/**
 * Marks a buffer replacement driven by the `value` prop. The parent already
 * holds that text, so reporting it back through `onChange` would be an echo —
 * and for a read-only preview it would wrongly register as an edit.
 */
const fromValue = Annotation.define<boolean>();

const highlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.operatorKeyword, tags.tagName], color: "var(--cm-keyword)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp], color: "var(--cm-string)" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], color: "var(--cm-number)" },
  { tag: [tags.comment, tags.meta], color: "var(--cm-comment)", fontStyle: "italic" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "var(--cm-function)" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.self], color: "var(--cm-type)" },
  { tag: [tags.propertyName, tags.attributeName, tags.definition(tags.propertyName)], color: "var(--cm-property)" },
  { tag: tags.heading, color: "var(--cm-function)", fontWeight: "600" },
  { tag: [tags.link, tags.url], color: "var(--link, var(--accent))" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.invalid, color: "var(--danger, #ff5d57)" },
]);

export function CodeEditor({
  value,
  filename,
  readOnly = false,
  wrap = false,
  onChange,
  onSelection,
  onSave,
}: {
  /**
   * The buffer's content. The editor is the source of truth while the user
   * types (each edit is reported through `onChange`); when `value` differs from
   * the buffer — a reload from disk, a conflict resolved to the disk copy — the
   * text is replaced in place, keeping the cursor and undo history.
   */
  value: string;
  /** Picks the language grammar (matched by name/extension, lazy-loaded). */
  filename: string;
  readOnly?: boolean;
  /** Soft-wrap long lines. */
  wrap?: boolean;
  onChange?: (doc: string) => void;
  onSelection?: (selection: EditorSelection | null) => void;
  /** ⌘S inside the editor. */
  onSave?: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  // Latest callbacks, read at event time so a new closure identity never
  // touches the editor.
  const cb = useRef({ onChange, onSelection, onSave });
  cb.current = { onChange, onSelection, onSave };
  // The view is built once per mount; everything that can change afterwards
  // lives in a compartment (reconfigured live) or is dispatched as a change.
  // Rebuilding on a prop change was the bug this replaces: the parent echoes
  // every keystroke back as `value`, and a rebuild resets the cursor to 0.
  const view = useRef<EditorView | null>(null);
  const [compartments] = useState(() => ({
    language: new Compartment(),
    wrap: new Compartment(),
    readOnly: new Compartment(),
  }));
  // Read by the build effect so the first state matches the current props
  // without listing them as deps.
  const props = useRef({ value, wrap, readOnly });
  props.current = { value, wrap, readOnly };

  useEffect(() => {
    if (!host.current) return;
    const editor = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: props.current.value,
        extensions: [
          basicSetup,
          theme,
          syntaxHighlighting(highlight),
          compartments.language.of([]),
          compartments.wrap.of(wrapExtension(props.current.wrap)),
          compartments.readOnly.of(EditorState.readOnly.of(props.current.readOnly)),
          // Word completion from the buffer, for every language — grammars
          // that ship real completions (html/css/…) add theirs on top.
          EditorState.languageData.of(() => [{ autocomplete: completeAnyWord }]),
          keymap.of([
            {
              key: "Mod-s",
              run: () => {
                cb.current.onSave?.();
                return true;
              },
            },
            indentWithTab,
          ]),
          EditorView.updateListener.of((update) => {
            const typed = update.transactions.some((tr) => !tr.annotation(fromValue));
            if (update.docChanged && typed) cb.current.onChange?.(update.state.doc.toString());
            if (update.selectionSet || update.docChanged) {
              const range = update.state.selection.main;
              cb.current.onSelection?.(
                range.empty
                  ? null
                  : {
                      code: update.state.sliceDoc(range.from, range.to),
                      start: update.state.doc.lineAt(range.from).number,
                      end: update.state.doc.lineAt(range.to).number,
                    },
              );
            }
          }),
        ],
      }),
    });
    view.current = editor;
    return () => {
      view.current = null;
      editor.destroy();
    };
  }, [compartments]);

  useEffect(() => {
    const editor = view.current;
    if (!editor || editor.state.doc.toString() === value) return;
    // Clamp rather than reset: after a reload the cursor stays near where the
    // user was instead of jumping to the top.
    const { anchor, head } = editor.state.selection.main;
    editor.dispatch({
      changes: { from: 0, to: editor.state.doc.length, insert: value },
      selection: { anchor: Math.min(anchor, value.length), head: Math.min(head, value.length) },
      annotations: fromValue.of(true),
    });
  }, [value]);

  useEffect(() => {
    view.current?.dispatch({ effects: compartments.wrap.reconfigure(wrapExtension(wrap)) });
  }, [compartments, wrap]);

  useEffect(() => {
    view.current?.dispatch({
      effects: compartments.readOnly.reconfigure(EditorState.readOnly.of(readOnly)),
    });
  }, [compartments, readOnly]);

  useEffect(() => {
    const description = LanguageDescription.matchFilename(languages, filename);
    if (!description) {
      view.current?.dispatch({ effects: compartments.language.reconfigure([]) });
      return;
    }
    let stale = false;
    description
      .load()
      .then((support) => {
        if (!stale) view.current?.dispatch({ effects: compartments.language.reconfigure(support) });
      })
      .catch(() => {
        /* no grammar for this file — plain text is fine */
      });
    return () => {
      stale = true;
    };
  }, [compartments, filename]);

  return <div ref={host} className="code-editor" />;
}

const wrapExtension = (wrap: boolean) => (wrap ? EditorView.lineWrapping : []);
