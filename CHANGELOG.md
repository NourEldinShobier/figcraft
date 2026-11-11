# Changelog

What changed in FigCraft's agent surface (the MCP server, the engine commands behind it, and the Claude Code plugin). Newest first. The full per-feature history is in the revision table of [ROADMAP.md](ROADMAP.md); tool parameters are in [docs/mcp.md](docs/mcp.md).

To use a new build: quit and restart the FigCraft app, then restart the agent session so its MCP server reloads. `diagnostics` shows `attached: true` when the app is driven, and `tools/list` should show 67 tools (full profile).

## 2026-11-11: upstream merge and bulk-edit release (54 to 67 tools)

The tree now includes upstream/main (644 commits merged).

New MCP tools:

- `design_system_index` (engine `ds.index`): components, styles and variables with id, name, description, `usageCount` and `unused`; `variantProperties` for component sets. Takes `limit`; says when truncated.
- `extract_text` and `apply_text` (engine `text.extract`, `text.apply`, `text.pseudoLocalize`): the text layers of a scope as an inventory, and the edited strings written back in one undo step.
- `get_board` (engine `board.read`) and `get_selection_context` (engine `selection.context`): board contents with connectors, and the selected layers with their parents and siblings. Read-only.
- `fix_contrast` (engine `a11y.fixContrast`) and `export_assets` (engine `export.assets`).
- `check_changes` (engine `check.changes`; `since`, `root`, `page`, `checks`, `limit`) and `visual_diff` (engine `visual.diff`; `since`, `frames`, `threshold`, `scale`): verify a bulk edit against a saved version. Read-only.
- `fill_instances` (data merge from `rows` or `csv`), `swap_component` (`mapOverrides`) and `replace_color` (`tolerance`, `variable`): bulk operations, each with `dryRun`.
- `changesets` (engine `changeset.begin`, `end`, `list`, `revert`): a change journal; a revert is one undo step, and `changed` summaries carry the open changeset label.

Changed:

- `batch` accepts `{"assert": {...}}` steps and `preview: true` (runs, reports, undoes).
- `figcraft-cli mcp --profile core|full` and `FIGCRAFT_MCP_PROFILE` (the flag wins, default `full`): core lists 21 tools plus `more_tools`, with short descriptions.
- Bulk writes take `maxAffected` (default 200), return a `changed` summary and honour the scope fence and owner locks. `ifRev` also guards `undo` and `redo`.
- `import_html` handles CSS grid, `position:absolute`, stretched flex rows, `@media` and `:first-child`/`:last-child`/`:nth-child(n)`.
- Stability: `run_script` stops after 20 s of command time; `generate_diagram` limits Mermaid subgraphs to 200, nested 16 deep; the session command log drops parameters over 16 KB; ten refusal texts say what to do.

To use it: quit and restart the FigCraft app, then restart the agent session. An older app that is still running rejects the new parameters (for example `maxAffected`) with "unknown parameters", so the app must be the new build.

Known limits:

- `cargo test -p figcraft-cli` attaches to a running FigCraft app on port 7979; with an older app open, `mcp_over_stdio` fails. Set `FIGCRAFT_MCP_PORT` to an unused port to run it headless.
- Boolean-group corner smoothing follows the merged upstream profile, so its area change is smaller than before (test threshold lowered from 1.0 to 0.5).
- `figcraft-web` tests were not run (excluded from the release gates).

Deferred: prototype flows (out of scope), a `description` input for `generate_diagram`.

## 2026-11-08: agent-tooling release (46 to 54 tools)

New MCP tools:

- `lint_layout` (engine `lint.layout`): overlap, misaligned edges, uneven gaps, tiny targets and (opt-in) off-grid positions. Each finding can carry a ready `fix` command. Takes `root`, `page`, `checks`, `grid`, `limit`.
- `check_accessibility` (engine `a11y.audit` and `a11y.checkTargets`, `a11y.checkNonText`, `a11y.checkAlt`, `a11y.readingOrder`): target size (24 px, 44 px in frames up to 500 wide), non-text contrast (3:1), missing alt text, and reading order. Alt text is set with `set_properties {props: {alt}}` (`""` marks a layer decorative, `null` clears it) and shows in `get_node`.
- `diff_documents` (engine `doc.diff`): structural diff of two open files, or of the current file against a saved version. Reports added, removed, moved and changed layers with property names and old and new values, capped by `limit`.
- `summarize_document` (engine `inspect.summary`): a token-budgeted overview (pages, top-level frames, component, style and variable counts, headings, repeated families, suggested next calls). `budgetTokens` defaults to 2000.
- `run_script` (engine `script.run`): a Rhai script with the host functions `cmd`, `find`, `get` and `log`. One undo step; it rolls back whole on a throw, a failed command or an exhausted budget (`maxOperations`). No file, network or clock access. `dryRun` reports and undoes.
- `import_html` (engine `import.html`): static HTML and CSS to editable layers, best effort. Block flow becomes vertical auto layout, flexbox becomes auto layout, `data:` images and inline SVG are imported. Nothing is fetched. What was dropped is listed in `warnings`. Limits: 2 MB, 5000 layers.
- `generate_diagram` (engine `board.fromMermaid`): Mermaid flowcharts, sequence diagrams and ER diagrams drawn as board shapes with attached connectors, in one section. Unsupported syntax is an error that names the line. The `description` input is not supported.
- `get_design_guidelines`: the Markdown rules stored with the file (`library.setGuidelines`, `library.guidelines`), plus the prompt `create_design_system_rules`.

Changed:

- `screenshot` takes `marks` and `markDepth`: numbered or id badges on layers and their children, with a legend in the text result, so a vision agent can name a layer.
- `get_design_context` takes `include` and returns more: `instances` (with Code Connect snippets), `annotations`, `devResources`, `status`, `bindings` (variable bindings), `interactions`, `comments` and `hints`. Old fields are unchanged. Defaults cover everything except `comments` and `interactions`.
- `export_tokens` adds formats: DTCG composite styles (typography, shadow, gradient, border), Style Dictionary, a resolver file, Swift, Kotlin and Android XML. The Tailwind export's variable names now match its CSS.
- Reads return `rev` (the document revision). Write tools accept `ifRev`; when it is stale the call changes nothing and says what moved, so an agent working from old context re-reads.
- The attach layer retries a full queue and a connection that closed before answering, with a short backoff; a lost reply is retried only for read-only methods. Errors are typed. `notifications/cancelled` is honoured for calls that can stop, and progress is sent when the client asks for it.

Licence note: `run_script` uses the `rhai` crate (MIT or Apache-2.0). Its dependency `smartstring` is MPL-2.0+; the owner approved it (see the comment in `deny.toml`).

## 2026-11-07: families, token audit, Code Connect, stability (41 to 46 tools)

- Families, for the same element on several screens: `find_similar`, `propagate_change` (a preview until `confirm: true`, one undo step), engine `family.set`, `family.list`. `set_properties` results on one to three layers carry `similarOn`, up to 5 close twins on other frames. Twins are scored on a family tag, shared component, structure, name and size. Propagation never copies names, text, positions or sizes unless `props` lists them.
- `audit_tokens` (engine `lint.tokens`): solid fills and strokes with a literal colour that a colour variable already holds, with `variable.bind` fixes.
- `get_code_connect_map` and `add_code_connect_map`: read and write Code Connect links with the import line and snippet.
- Prompts `apply_everywhere` and `design_to_code`.
- Stability: after the app restarts, the next call notices, clears the target document and says ids from before are void. The first call after a restart now carries the fresh control token (before, it was refused). A result over 8 MiB of text or 16 MiB of image data becomes an error that says how to ask for less. `list_commands` takes `limit` (default 400) and `offset`. Layer ids may be written `#12` or `figcraft://node/12`.

## 2026-11-07: tabs, files, versions, layout (the first agent batch)

- Agent calls never switch the user's tab, zoom or window. `open_file` adds the file as a background tab; `open_file {focus: true}` switches on purpose.
- `use_document` sets the file later calls act on; `document` and `documents: "all"` (or a list) name files per call on `find_nodes`, `get_nodes`, `set_properties` and `run_command`. `copy_between_documents` copies layers across open files.
- `checkpoint`, `list_versions`, `restore_version`, `compare_version`.
- Auto-save is on by default and the MCP turns it on when it attaches (`FIGCRAFT_MCP_NO_AUTOSAVE=1` skips that).
- Starting FigCraft with a file while it runs adds the file as a tab in the running window instead of opening a second one.
- `find_nodes` filters `page`, `widthNot`, `heightNot`, `outsideParent`, `edgeGap` and `details`; `inspect.overflow`; every command takes `where`, `list`, `dryRun` and `keepSelection`; named id lists (`list.save`, `list.get`, `list.delete`, `list.all`); `node.adjust`; `get_nodes`, `contact_sheet`, `get_selection`, `diagnostics`, `edit.replaceColor`; `save_file` reports `unsavedBefore` and `changedOnDisk`.
- Layout: aspect-ratio lock inside auto layout, variable-mode breakpoints that reflow a frame, `layout.previewWidths`, Fixed and Sticky scroll refused on auto-layout flow children, and Dev Mode code for grid, min/max, Fill, absolute children and `aspect-ratio`.

## Known limits

- `diagnostics` shows `path: null`: file paths are hidden from agents.
- One shared clipboard: two agents copying at the same moment can paste each other's layers.
- Twin scoring can rate unrelated layers of the same name and size high; tag them with `family.set` or pass `ids` to `propagate_change`.
- `import_html`: no CSS grid, `position: absolute`, transforms, media queries or pseudo-elements.
- `generate_diagram`: edges that skip several ranks can pass behind nodes; generated diagrams were checked in tests, not visually.
- `lint_icons` rejects the `cap`, `join` and `color` parameters in this build although the schema lists them.
- Not built: motion (keyframes, timeline, animated export), 3D transforms, Mermaid-free `description` input for `generate_diagram`, mid-call progress values and interrupting a running backend call on cancel.
- The new tools were tested headless and against a fake app; they have not been exercised in a long live session with the app window.
