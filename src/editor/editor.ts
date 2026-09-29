// Lazy CodeMirror entry point (spec 005, AC-005-03). Nothing in src statically imports
// CodeMirror: this module is reached only through `import("../../editor/editor")` when the
// first editable tab becomes ready, so the editor and language packages never load during
// GUI bootstrap. Language packages are loaded per file through @codemirror/language-data.

export interface EditorOptions {
  /**
   * Spec 006: remote tabs are read-only (no edits through keys, paste or commands). Absent or
   * false keeps the editable editor of the local files feature (spec 005).
   */
  readOnly?: boolean;
}

/** State/view extensions for `options`; empty for the default editable editor. */
export async function editorOptionExtensions(
  options: EditorOptions,
): Promise<import("@codemirror/state").Extension[]> {
  if (options.readOnly !== true) return [];
  const { EditorState } = await import("@codemirror/state");
  const { EditorView } = await import("@codemirror/view");
  return [EditorState.readOnly.of(true), EditorView.editable.of(false)];
}

export interface EditorHandle {
  /**
   * Shows `text` for `tabId` and reports every doc change. The per-tab state is reused while
   * its text matches (typing keeps cursor/undo; switching tabs preserves them), and replaced
   * when the buffer changed from outside (initial load, reload, conflict reload).
   */
  show(
    tabId: string,
    text: string,
    filename: string,
    onChange: (text: string) => void,
  ): Promise<void>;
  focus(): void;
  destroy(): void;
}

async function languageFor(filename: string): Promise<import("@codemirror/state").Extension[]> {
  const { LanguageDescription } = await import("@codemirror/language");
  const { languages } = await import("@codemirror/language-data");
  const description = LanguageDescription.matchFilename(languages, filename);
  if (!description) return [];
  try {
    return [await description.load()];
  } catch {
    // A language package failure must never block plain-text editing.
    return [];
  }
}

export async function mountEditor(
  parent: HTMLElement,
  options: EditorOptions = {},
): Promise<EditorHandle> {
  const { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } = await import(
    "@codemirror/view"
  );
  const { EditorState } = await import("@codemirror/state");
  const { defaultKeymap, history, historyKeymap, indentWithTab } = await import(
    "@codemirror/commands"
  );

  const view = new EditorView({ parent });
  const states = new Map<string, import("@codemirror/state").EditorState>();
  const languageCache = new Map<string, import("@codemirror/state").Extension[]>();

  async function extensionsFor(
    filename: string,
    onChange: (text: string) => void,
    tabId: string,
  ): Promise<import("@codemirror/state").Extension[]> {
    let language = languageCache.get(filename);
    if (!language) {
      language = await languageFor(filename);
      languageCache.set(filename, language);
    }
    return [
      lineNumbers(),
      highlightActiveLine(),
      drawSelection(),
      history(),
      keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
      EditorView.lineWrapping,
      EditorView.updateListener.of((update) => {
        if (!update.docChanged) return;
        states.set(tabId, update.state);
        onChange(update.state.doc.toString());
      }),
      ...language,
      ...(await editorOptionExtensions(options)),
    ];
  }

  return {
    async show(tabId, text, filename, onChange) {
      const cached = states.get(tabId);
      if (cached && cached.doc.toString() === text) {
        if (view.state !== cached) view.setState(cached);
        view.focus();
        return;
      }
      const extensions = await extensionsFor(filename, onChange, tabId);
      const state = EditorState.create({ doc: text, extensions });
      states.set(tabId, state);
      view.setState(state);
      view.focus();
    },
    focus() {
      view.focus();
    },
    destroy() {
      states.clear();
      languageCache.clear();
      view.destroy();
    },
  };
}
