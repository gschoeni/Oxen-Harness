// The CodeMirror 6 wrapper: syntax highlighting (language picked from the
// filename, grammar lazy-loaded), autocomplete (language completions where the
// grammar provides them, buffer-word completion everywhere), and the standard
// editing chrome (line numbers, search, bracket matching) via basicSetup.
// Colors come from CSS variables (defined in files.css) so the editor follows
// the app's theme and light/dark mode for free.

import { useEffect, useRef } from "react";
import { basicSetup, EditorView } from "codemirror";
import { Compartment, EditorState } from "@codemirror/state";
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
  initial,
  filename,
  readOnly = false,
  onChange,
  onSelection,
  onSave,
}: {
  /** The buffer's starting content; changing it rebuilds the editor. */
  initial: string;
  /** Picks the language grammar (matched by name/extension, lazy-loaded). */
  filename: string;
  readOnly?: boolean;
  onChange?: (doc: string) => void;
  onSelection?: (selection: EditorSelection | null) => void;
  /** ⌘S inside the editor. */
  onSave?: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  // Latest callbacks without rebuilding the editor when they change identity.
  const cb = useRef({ onChange, onSelection, onSave });
  cb.current = { onChange, onSelection, onSave };

  useEffect(() => {
    if (!host.current) return;
    const language = new Compartment();
    const view = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: initial,
        extensions: [
          basicSetup,
          theme,
          syntaxHighlighting(highlight),
          language.of([]),
          EditorState.readOnly.of(readOnly),
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
            if (update.docChanged) cb.current.onChange?.(update.state.doc.toString());
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

    let disposed = false;
    const description = LanguageDescription.matchFilename(languages, filename);
    description
      ?.load()
      .then((support) => {
        if (!disposed) view.dispatch({ effects: language.reconfigure(support) });
      })
      .catch(() => {
        /* no grammar for this file — plain text is fine */
      });

    return () => {
      disposed = true;
      view.destroy();
    };
  }, [initial, filename, readOnly]);

  return <div ref={host} className="code-editor" />;
}
