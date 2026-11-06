# Layout and responsive design: gap analysis

> **Reviewed:** 2026-11-06 · **Scope:** Figma Design layout as documented publicly (help center, plugin API docs, release notes) against FigCraft at this commit · **Method:** public pages read for the rules; FigCraft status from code and test greps, no cargo run. Anything not confirmed in code or docs is marked *unverified*.

## Summary

1. FigCraft can build flexbox-like responsive layouts today: horizontal/vertical/wrap/grid auto layout, Hug/Fill/Fixed, min/max, ignore-auto-layout with constraints, constraints, the Scale tool and layout grids are implemented, and the auto layout rules are covered by 64 layout cases plus engine command tests ([auto-layout-parity.md](auto-layout-parity.md)).
2. It already follows Figma's current (July 2026) CSS-aligned auto layout version only: padding floor, inside-only strokes, border-box fill sharing, auto gap never below 0. There is no per-frame legacy/new version, so older Figma files that stay on the legacy version may reflow differently on import (unverified, no real file checked).
3. A designer can resize a frame and watch content reflow (`transform.resize`, W/H, handles; Ctrl/Cmd ignores constraints). There is no non-destructive way to see one frame at several widths, and Figma's public docs describe none either; the documented Figma way is a frame per breakpoint driven by a number-variable mode.
4. Real gaps: aspect-ratio lock is not honoured by the layout engine, Inspect CSS emits flex for grid frames and omits min/max/fill/border-box, `Fixed` scroll position is not restricted to ignore-auto-layout children, and variable-mode-driven size changes are untested for relayout.
5. Ranked gaps with estimates are at the end (top item: breakpoint modes verify/fix, 4-9 h; preview widths 8 h; aspect-ratio lock 12 h; CSS grid/min/max/fill 8 h).

## Sources

| Tag | Page |
|---|---|
| A | Guide to auto layout, https://help.figma.com/hc/en-us/articles/360040451373-Guide-to-auto-layout |
| B | Use the horizontal and vertical flows, https://help.figma.com/hc/en-us/articles/31289464393751 |
| C | Use the grid auto layout flow, https://help.figma.com/hc/en-us/articles/31289469907863-Use-the-grid-auto-layout-flow |
| D | Use auto layout with CSS Flexbox in mind, https://help.figma.com/hc/en-us/articles/42031586813719-Use-auto-layout-with-CSS-Flexbox-in-mind |
| E | Apply constraints, https://help.figma.com/hc/en-us/articles/360039957734-Apply-constraints-to-define-how-layers-resize |
| F | Scale layers while maintaining proportions, https://help.figma.com/hc/en-us/articles/360040451453-Scale-layers-while-maintaining-proportions |
| G | Prototype scroll and overflow behavior, https://help.figma.com/hc/en-us/articles/360039818734-Prototype-scroll-and-overflow-behavior |
| H | Set prototype device and background settings, https://help.figma.com/hc/en-us/articles/21158597546391-Set-prototype-device-and-background-settings (search summary only, page body not read) |
| I | Combine vertical, horizontal and grid flows, https://help.figma.com/hc/en-us/articles/31441443713047-Combine-vertical-horizontal-and-grid-auto-layout-flows (summary only) |
| J | Plugin API `targetAspectRatio`, https://developers.figma.com/docs/plugins/api/properties/nodes-targetaspectratio/ and update 107, https://www.figma.com/plugin-docs/updates/2025/02/13/version-1-update-107/ |
| K | Guide to variables (frame switched between device sizes), https://help.figma.com/hc/en-us/articles/15339657135383-Guide-to-variables-in-Figma ; `resolvedVariableModes`, https://developers.figma.com/docs/plugins/api/properties/nodes-resolvedvariablemodes/ |
| N | Release notes, https://www.figma.com/release-notes/ (front page shows AI and Make items only; the layout-version release is read from D and secondary posts, date 2026-07-24 *unverified*) |

Not researched in depth: Layout grids article, Figma Sites breakpoints, Dev Mode code output for grid (secondary posts say CSS `display: grid`; D says Dev Mode maps to flexbox), text trim, Resize-to-fit. Marked *unverified* below.

## Checklist

Status: **done+T** implemented and tested; **done** implemented, no test found; **partly**; **missing**; **n/a**; **unverified**. Priority: H/M/L for a designer building responsive screens.

| Feature | Figma behaviour | Src | Status | Evidence | Pri |
|---|---|---|---|---|---|
| Horizontal / vertical flow | Objects follow the x or y axis; add/remove/reorder | A,B | done+T | `crates/layout/src/behaviour_tests.rs` (`sizing_rules`, `alignment_rules`) | - |
| Wrap, both directions | New row/column when space runs out; vertical wrap is not masonry; row gap separate | A,B | done+T | `wrap_rules`, `content_align_tests.rs` (13 cases), `al_content_align_tests.rs` | - |
| Grid flow: rows, columns, gaps, padding | Tracks fixed / hug / `fr`; rows Auto grow and shrink | C | done+T | `grid_rules`, `grid_place_tests.rs`, `layout.setTrack` | - |
| Grid: spans, cell alignment, move/swap | Cell objects span tracks; swap by smart selection | C | done+T (swap UI unverified) | `ChildLayout.grid_*` in `crates/doc/src/props.rs`, `al_drag_tests.rs` | L |
| Grid: reorder/delete tracks, math in counts | Drag grabber, Delete key, `3*2` in count fields | C | unverified | not grepped in `crates/ui-egui` | L |
| Hug / Fill / Fixed | Rules incl. Fill turning a hugging parent Fixed; no Fill on top-level | A | done+T | `sizing_rules`, `al_behaviour_tests::fill_on_a_child_turns_the_hugging_parent_fixed` | - |
| Resize to full space becomes Fill; double-click edge Hug, Alt+double-click Fill | Gesture shortcuts | A | done+T | `al_behaviour_tests`, `double_click_tests` (parity doc rows) | - |
| Min/max width and height | Additional to Hug/Fill/Fixed; remove all; may take a number variable | A,C | done+T | `ChildLayout.min_w..max_h`; `sizing_rules`; scopes `minWidth..maxHeight` in `crates/engine/src/cmd/variables.rs:627` | - |
| Padding per side, CSS shorthand | `1,2,3,4` in field; Alt/Shift drag handles | A,B | done+T | `padding_and_stroke_rules`; shorthand and handles: parity doc row 90, shorthand parse *unverified* | L |
| Gap number, auto gap Between/Evenly/Around | Maps to CSS `space-*`; stops at 0; single child at start for Between, centred for Around/Evenly | B,D | done+T (Around/Evenly single child *unverified*) | `gap_rules`, `MainAlign` in `props.rs:1043` | L |
| Alignment 3x3, baseline | Nine positions; only cross axis with auto gap | B | done+T | `alignment_rules` | - |
| Padding floor, inside strokes only, border-box Fill share | New layout version rules | D | done+T | auto-layout-parity rows 41, 53-56 | - |
| Stroke setting no longer inherited by child frames | Parent setting affects only its own frame | D | unverified | `AutoLayout.strokes_included` is per frame; no test of a child with different setting | L |
| Layout version (legacy/new) per frame, upgrade actions | Existing frames stay legacy until January 2027; instances cannot override | D,N | missing | no field in `crates/doc` or the `.fig` export schema; only new behaviour implemented | M |
| Nested auto layout frames | Each frame has own flow, gap, padding | A,I | done+T | `nested_tests.rs`, `al_nesting_tests.rs` | - |
| Ignore auto layout (absolute) | Out of flow, stays in frame; constraints apply; other AL settings unavailable | A | done+T | `absolute_position_rules`, `ChildLayout.absolute` | - |
| Canvas stacking last/first on top | Setting in auto layout menu | G | done+T | `AutoLayout.reverse_z`, parity doc row 68 | - |
| Text: hug, fill, fixed, max lines vs max height | Fixed text overlaps in fluid layouts; max height XOR max lines | A | done+T | `TextAutoResize` in `crates/doc/src/text.rs:214`; `text_behaviour_tests::a_text_has_a_max_height_or_max_lines_not_both` | - |
| Constraints: Left, Right, L+R, Center, Scale (and vertical) | Reposition/resize children when the frame resizes; default Top+Left; Scale keeps percentage | E | done+T | `HConstraint`/`VConstraint`, `transform_behaviour_tests::frames_resize_with_constraints_and_cmd_ignores_them` | - |
| Ctrl/Cmd while resizing ignores constraints and auto layout resizing | Temporary override | E,G | partly | constraints ignored (tested); ignoring AL resizing properties *unverified* | L |
| Constraints on groups | Applied to the group's layers | E | unverified | `groups_scale_children_keeping_their_rotation_and_flip` covers scale, not constraints on a group set via UI | L |
| Constraints not on AL children unless ignored | Hidden for AL children | A,E | done | parity doc "constraints apply only then" | - |
| Scale tool (K) | Proportional, ignores nested constraints, scales strokes/blur, not locked layers or layers inside instances | F | done+T | `tool.scale`, `tool.scaleAnchor`, `crates/engine/src/scale_tool_tests.rs` | - |
| Lock aspect ratio | Stored ratio (`targetAspectRatio`); kept when resized via auto layout, constraints, panel, canvas | J | partly | `Node.constrain_proportions` (bool, `crates/doc/src/node.rs:517`): honoured by panel W/H (`cmd/props.rs:541`) and canvas drag (`canvas/tools/drag.rs:520`); no use in `crates/layout`, no stored ratio | H |
| Clip content | Hides overflow of a frame | G | done | `Node.clip` | - |
| Instances: padding and gap editable, layers not added/reordered | Instance overrides | A | unverified | `instance_resizing_tests.rs` covers sizing overrides only | M |
| Instance resizing (Fill/Hug/Fixed overrides survive main edits) | Responsive components | A | done+T | `instance_resizing_tests.rs` (5 cases) | - |
| Component variants as breakpoints | Variant per size | K (tutorial summary only) | done | component sets and variants exist, no responsive-specific test | L |
| Variables for layout values | Number variables bind to width, height, min/max, gap, padding | A,K | done+T | `variables_behaviour_tests.rs:250-304` (width, gap, padding binding) | - |
| Explicit variable mode per frame ("switch frame between device sizes") | A mode set on a frame is inherited by descendants; spacing and sizes update at once | K | partly | `variable.setMode`, `Node.var_modes`, tests for colours (`variables_behaviour_tests.rs:194-322`); relayout after a mode changes width/gap *unverified* | H |
| Variable-bound sizes in the layout pass | Frame width of 375 vs 1440 per mode reflows Fill children | K | unverified | no test found that sets a mode and checks a Fill child's width | H |
| Preview a frame at other widths without editing | Not documented by Figma; practice is a frame per breakpoint or resizing | A,K | missing | no command in `commands` output; `text.preview` (`commands.txt:729`) shows a no-undo preview pattern to reuse | M |
| Frame presets / devices | Device list when creating frames | H | done | `FRAME_PRESETS` in `crates/engine/src/cmd/create.rs:15` (Phone, Tablet, Desktop, 19+ rows), `frame.presets` | - |
| Prototype device frame, orientation, scale (fit, fit width, fill, actual) | Device container in presentation | H | done | the prototype device-settings and presenter-scale commands (`commands.txt` lines 970-978), `crates/ui-egui/src/present/{device,layout}.rs` | - |
| Prototype overflow scrolling (none, horizontal, vertical, both) | Needs content larger than frame | G | done+T | `Overflow` in `crates/doc/src/proto.rs:828`, `present/scroll.rs:135-156`, `present/behaviour_tests.rs` | - |
| Scroll position Scroll with parent / Fixed / Sticky | Fixed moves above siblings; sticky sticks at frame top, vertical frames only | G | done (Sticky untested) | `ScrollPosition`, `present/scroll.rs:289-301`, test `fixed_layers_stay_above_the_content_that_scrolls`; no sticky test found | M |
| Fixed needs ignore-auto-layout inside an auto layout frame | Cannot assign Fixed otherwise | G | missing | `cmd/props.rs:900` sets `scrollPosition` without a check; no guard in `proto_panel.rs:1315` | M |
| "Content needs to be bigger than the frame" scroll warning | Error icon on overflow setting | G | unverified | not grepped | L |
| Preserve/reset scroll position between frames | Setting on navigation | G | unverified | page text cut off before this section | L |
| Layout grids columns/rows/grid, stretch/left/center/right, variables | Guides, not layout | E (link) | done+T | ui-parity row 114, `GridKind`, `GridAlign` in `props.rs:1234-1249` | - |
| Percent-like sizing | No percent unit; Scale constraint (percent of frame), grid `fr`, Fill+max | E,C | n/a (equivalents done) | `HConstraint::Scale`, `GridTrack::fill` | - |
| Dev Mode CSS for flex frames | Auto layout maps to flexbox (D FAQ) | D | done+T | `crates/engine/src/dev/codegen/css.rs:186-234`, `html_codegen_tests.rs`, `tailwind_tests.rs` | - |
| Dev Mode CSS for grid frames | `display: grid` with tracks (secondary sources) | C | missing | `layout_decls` emits `display: flex; flex-direction: row` for `LayoutMode::Grid`; no `grid-template` anywhere under `crates/engine/src/dev` | H |
| Dev Mode CSS for min/max, Fill, ignore-AL, border-box, aspect ratio | Maps to `min-width`, `flex`, `position: absolute`, `box-sizing` | D | partly | `html.rs:152-170` emits `flex: 1 1 0`, `align-self: stretch`, `position: absolute` for whole-tree HTML; per-node `css.rs` always emits fixed `width`/`height` px, no min/max, no `box-sizing`, no `aspect-ratio` | H |
| Figma Sites responsive breakpoints | Site breakpoints | - | unverified (not researched) | - | L |
| Slides/Buzz auto layout | Same engine in other file types | A | n/a | FigCraft is a Design clone | - |

## Gaps, ranked

Hours are for one engineer including tests and the parity-doc rows. Layer rules from `AGENTS.md` apply (layout crate stays below engine; UI thin).

### 1. Responsive via variable modes: verify, then fix (H, 4-9 h)
- **Build:** a test and, where it fails, a fix so `variable.setMode` on a frame re-runs layout when a number variable bound to width, height, gap, padding or min/max changes value; the whole change is one undo step.
- **Touches:** `crates/engine/src/cmd/variables.rs` (setMode and binding application), `crates/layout/src/lib.rs` call site, `crates/engine/src/variables_behaviour_tests.rs`.
- **Acceptance:** collection `Screen` with modes Mobile and Desktop; number variable `w` = 375 / 1440 bound to a horizontal auto layout frame's `width`; padding 16 on both sides; one Fill text child. After `variable.setMode {ids:[frame], collection:"Screen", mode:"Mobile"}` the child width is 343; after `Desktop` it is 1408; one `undo` returns to 1408 or 343 as before the call.

### 2. Preview a frame at other widths without changing the design (M, 8 h command, +12 h interactive)
- **Build:** `layout.previewWidths {ids, widths:[..], path?}`: for each width, clone the frame into a scratch document, resize it with constraints and auto layout, and return/export one PNG with the variants side by side (reuse the contact-sheet code). Phase two: a width scrubber on the frame in the canvas using the `text.preview` no-undo mechanism, with preset chips from `FRAME_PRESETS`.
- **Touches:** new `crates/engine/src/cmd/layout_preview.rs`, `crates/engine/src/cmd/layout.rs`, `crates/render` (sheet), later `crates/ui-egui/src/canvas`, `panels/right.rs`. Register in `crates/engine/src/catalog.rs` only if Figma has a matching command (it does not; keep it as an extra).
- **Acceptance:** frame 1440 wide, horizontal wrap, hug height, gap 20, padding 0, five 300x100 children. Previews at 375 / 768 / 1440 report 5 / 3 / 2 rows and heights 580 / 340 / 220; the source frame stays 1440 wide and the undo stack length is unchanged.

### 3. Lock aspect ratio honoured by layout and constraints (H, 12 h)
- **Build:** store the ratio (`target_aspect: Option<f64>` next to `constrain_proportions`, set when locking, per J); in the layout pass, when a locked child is Fill on one axis, derive the other axis from the ratio (min/max still clamp, then the ratio is re-applied to the unclamped axis); apply the same in constraint-driven resize (Left+Right makes the height follow). Read/write `.fig` and REST only if the field exists in the repo's own schema notes (otherwise leave unmapped).
- **Touches:** `crates/doc/src/node.rs`, `crates/layout/src/lib.rs`, `crates/engine/src/cmd/props.rs:541`, `crates/engine/src/canvas/tools/drag.rs:520`, `crates/ui-egui/src/panels/right.rs` (lock toggle), format crates.
- **Acceptance:** horizontal auto layout frame Fixed 300 wide, padding 0; child 200x100 locked, `sizingH` Fill, `sizingV` Hug. After layout the child is 300x150. Resizing the frame to 450 gives 450x225.

### 4. Dev Mode CSS: grid frames and sizing rules (H, 6-9 h)
- **Build:** in `css.rs` emit `display: grid`, `grid-template-columns/rows` (fixed `Npx`, hug `auto`, fill `Nfr`), `column-gap`/`row-gap`, child `grid-column: a / span n`; emit `min-width/max-width/min-height/max-height`, `flex: 1 1 0` / `align-self: stretch` for Fill children and `position: absolute` for ignore-auto-layout children in the per-node output as `html.rs` already does for trees; add `box-sizing: border-box` on auto layout frames (D) and `aspect-ratio` once gap 3 lands. Mirror in `tailwind.rs`.
- **Touches:** `crates/engine/src/dev/codegen/css.rs`, `html.rs`, `crates/engine/src/dev/tailwind.rs`, tests in `html_codegen_tests.rs`, `tailwind_tests.rs`; the property-name check for other languages in `flutter.rs`/`compose.rs`/`swift.rs` is *unverified* and left out.
- **Acceptance:** grid frame with columns Fixed 100, Fill 1fr, Fill 2fr and gap 8 gives `display: grid;` `grid-template-columns: 100px 1fr 2fr;` `column-gap: 8px;` (and no `display: flex`). A child with min width 200 and max width 400 gives `min-width: 200px;` and `max-width: 400px;`.

### 5. Restrict Fixed scroll position in auto layout frames (M, 2-3 h)
- **Build:** reject `scrollPosition: fixed` on a child of an auto layout frame unless `absolute` is set (G); disable the Fixed item in the Prototype panel for such layers; when a fixed child later stops ignoring auto layout, reset to Scrolls with a warning.
- **Touches:** `crates/engine/src/cmd/props.rs:900`, `crates/ui-egui/src/panels/proto_panel.rs:1315`, `crates/engine/src/cmd/layout.rs`.
- **Acceptance:** `node.set {ids:[child], props:{scrollPosition:"fixed"}}` on an auto layout child returns an error; the same call after `absolute:true` succeeds and the child's `scroll_position` is `Fixed`.

### 6. Sticky position test and unverified documented rules (M, 3-4 h, tests only unless a test fails)
- **Build:** tests for: sticky only in vertical-scroll frames and clamped to the parent's bounds (G); single child with Around/Evenly stays centred (B); child frame stroke setting independent of the parent's (D); Ctrl while resizing a frame ignores auto layout resizing (G).
- **Touches:** `crates/ui-egui/src/present/behaviour_tests.rs`, `crates/layout/src/behaviour_tests.rs`, `crates/engine/src/transform_behaviour_tests.rs`.
- **Acceptance:** vertical-scroll frame 100 tall with content 300 tall, sticky child 20 tall at y=40: scrolled by 60 the child's screen y is 0; scrolled by 20 it is 20. Single 40-wide child in a 100-wide horizontal frame, padding 0, auto gap Around: x = 30.

### 7. Instance rules for auto layout (M, 3 h tests, +3 h if rules are missing)
- **Build:** tests that an instance accepts padding/gap overrides but refuses reorder/add, and that deleting a layer in an instance hides it only (A table).
- **Touches:** `crates/engine/src/instance_resizing_tests.rs` (or a new `al_instance_tests.rs`), `crates/engine/src/assets/components`.
- **Acceptance:** main component frame with padding 10; instance override padding 24 gives a hugging instance 28 px wider than before the override (2 x 14); moving a layer inside the instance (reorder) returns an error. Both statuses are *unverified* today.

### 8. Legacy vs new layout version on import (M, 4-6 h, blocked on a sample)
- **Build:** a per-frame `layout_version` read from `.fig`/REST when present, with the legacy rules (padding may be squeezed, all strokes counted per setting, fill shares by outer size, auto gap may go negative) behind it, and an "Update layout version" action for selection and page (D). Needs an observed real file (synthetic, from the incumbent app by black-box use) to learn the field; do not read the app bundle.
- **Touches:** `crates/doc/src/props.rs` (`AutoLayout`), `crates/layout/src/lib.rs`, `crates/format/src/{fig,rest}`, `crates/engine/src/catalog.rs`.
- **Acceptance:** legacy frame Fixed 50 wide with horizontal padding 30 per side lays out at 50 wide; after the update action it is 60 wide.

### 9. Smaller items (L)
- Track operations in grid frames: reorder, delete, `3*2` in count fields, Auto rows drop empty rows (C): 4 h tests/fixes, `layout.setTrack` and `crates/ui-egui/src/canvas`.
- Constraints on a group applied to its layers (E): 1-2 h test.
- "Content needs to be bigger than the frame" warning and preserve/reset scroll position on navigation (G): 3 h, `proto_panel.rs`, `present/navigate.rs`.
- Figma Sites breakpoints, Dev Mode grid output in Flutter/Compose/Swift: not researched.

## Notes for the implementer

- The repo's parity document already cites D and A for every auto layout rule above marked done+T; update [auto-layout-parity.md](auto-layout-parity.md) rows and the revision table in the same commit as each gap.
- Do not add Figma-derived assets or copy text from the pages above; the sources are cited for behaviour only.
- Left unchecked: nothing here was run (no cargo, no app); statuses come from reading code, test names and `figcraft-cli commands`.
