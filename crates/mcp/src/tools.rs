//! MCP tool definitions (names, descriptions, JSON Schemas) and their implementations on top of a
//! [`Backend`]. Every tool reduces to control-channel methods, so headless and attached sessions
//! behave the same.

use base64::Engine as _;
use figcraft_engine::params::normalize_paint;
use serde_json::{Map, Value, json};

use crate::backend::{Backend, decode_data, page_children};

/// The result of `tools/call`: MCP content blocks plus the `isError` flag.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(t: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": t.into()})], is_error: false }
    }
    pub fn json(v: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(v).unwrap_or_default())
    }
    pub fn error(t: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": t.into()})], is_error: true }
    }
    /// An image block followed by a JSON text block describing it.
    pub fn image(bytes: &[u8], mime: &str, info: &Value) -> Self {
        Self {
            content: vec![
                json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": mime}),
                json!({"type": "text", "text": info.to_string()}),
            ],
            is_error: false,
        }
    }
    pub fn to_value(&self) -> Value {
        json!({"content": self.content, "isError": self.is_error})
    }
    /// The first text block (tests and the CLI).
    pub fn first_text(&self) -> &str {
        self.content.iter().find_map(|c| c.get("text").and_then(Value::as_str)).unwrap_or("")
    }
}

// ---------- schemas ----------

fn num(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}
fn int(desc: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": desc})
}
fn string(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}
fn boolean(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}
fn id_schema(desc: &str) -> Value {
    json!({"description": desc, "anyOf": [{"type": "integer"}, {"type": "string"}]})
}
fn ids_schema(desc: &str) -> Value {
    json!({"type": "array", "description": desc, "items": {"anyOf": [{"type": "integer"}, {"type": "string"}]}})
}
fn fields_schema() -> Value {
    json!({"type": "array", "items": {"type": "string", "enum": figcraft_engine::inspect::NODE_KEYS},
        "description": "Exact, case-sensitive top-level node keys to include at every level, pages too (e.g. name, type, x, fills, characters, pluginData). Omit for all properties; [] keeps only ids and structure. id, children, childCount and the transport's truncated marker are always kept. Selecting visible emits an explicit boolean for every node. Unknown keys are an error."})
}
/// `fields` of `get_nodes`: [`fields_schema`] with a small default.
fn nodes_fields_schema() -> Value {
    let mut s = fields_schema();
    s["description"] = json!(
        "Exact, case-sensitive top-level node keys to include for every layer and child (default [\"name\", \"type\", \"x\", \"y\", \"width\", \"height\"]; any key of get_node, e.g. fills, characters, layout). id, children and childCount are always kept. [] keeps only those. Unknown keys are an error."
    );
    s
}
fn outline_schema() -> Value {
    boolean(
        "Compact tree: id, name, type, characters (first 120 characters, then ... only if longer), mainComponentName and explicit visible booleans, plus structure. Cannot combine true with fields; false permits fields.",
    )
}
/// `where` for `select`, `set_properties`, `get_nodes`, `contact_sheet` and the generic `where` of
/// every command: the `node.find` filters, without `details` (an output option of `find_nodes`).
fn where_schema() -> Value {
    let mut props = find_props();
    if let Some(o) = props.as_object_mut() {
        o.remove("details");
    }
    obj(props, &[])
}

/// Every `node.find` argument (`find_nodes` takes all, `where` all but `details`).
fn find_props() -> Value {
    let range = |desc: &str| json!({"description": desc, "anyOf": [{"type": "number"}, obj(json!({"min": {"type": "number"}, "max": {"type": "number"}}), &[])]});
    let gap = obj(json!({"edge": string("left | right | top | bottom"), "min": {"type": "number"}, "max": {"type": "number"}}), &[]);
    json!({
            "root": id_schema("Search inside this layer's subtree (default: the current page)"),
            "page": id_schema("Search this page (id or name) or \"all\" for every page (default: the current page)"),
            "widthNot": range("Layers whose width is NOT this width or {min?, max?}"),
            "heightNot": range("Layers whose height is NOT this height or {min?, max?}"),
            "outsideParent": boolean("Only layers whose box reaches outside their parent's box"),
            "edgeGap": {"description": "The gap between the layer's edge and its parent's same edge: {edge, min?, max?} or a list of them (all must hold)", "anyOf": [gap.clone(), {"type": "array", "items": gap}]},
            "details": boolean("Also return `nodes`: [{id, name, type, parent, x, y, width, height}] for the matches (x/y relative to the parent)"),
            "name": string("Case-insensitive substring of the layer name"),
            "nameRegex": string("Case-insensitive regular expression, matched anywhere in the name"),
            "nameGlob": string("`*` and `?` against the whole name, case-insensitive"),
            "type": string("Layer type: PAGE, FRAME, GROUP, SECTION, RECTANGLE, ELLIPSE, POLYGON, STAR, LINE, VECTOR, TEXT, BOOLEAN_OPERATION, COMPONENT, COMPONENT_SET, INSTANCE or SLICE (case-insensitive)"),
            "text": string("Case-insensitive substring of a text layer's characters"),
            "component": string("Case-insensitive substring of the main component's name, for instances"),
            "fill": string("A colour like \"#111111\": layers with a visible solid fill of it"),
            "stroke": string("A colour like \"#111111\": layers with a visible solid stroke of it"),
            "strokeWeight": range("A stroke weight, or {min?, max?} (inclusive)"),
            "width": range("A width, or {min?, max?} (inclusive)"),
            "height": range("A height, or {min?, max?} (inclusive)"),
            "parent": id_schema("Only the direct children of this layer"),
            "variant": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Variant properties, e.g. {\"State\": \"Hover\"}: variant components or their instances"},
            "key": string("The layer with exactly this key"),
            "keyGlob": string("Layers whose key matches this `*`/`?` pattern (case-sensitive)"),
            "limit": int("Most layers to take (default 100, up to 10000); a filter that matches more is an error: narrow it or raise the limit"),
    })
}

/// `params` of `run_command` and of a `batch` step: any command key, and the generic parameters
/// every command takes, listed so that they are documented (the engine checks the rest).
fn command_params_schema() -> Value {
    let mut w = where_schema();
    w["description"] = json!("Act on the layers these node.find filters match instead of `ids`/the selection");
    let mut s = obj(
        json!({
            "where": w,
            "list": string("Act on the layers of this saved list (list.save / list.get) instead of `ids`/the selection"),
            "dryRun": boolean("Change nothing; return {dryRun, wouldChange, created, deleted, count, result}"),
            "keepSelection": boolean("Leave the user's selection and view as they are"),
        }),
        &[],
    )
    .with_open_keys();
    s["description"] =
        json!("Command parameters (see list_commands). Every command also takes the generic `where`, `list`, `dryRun` and `keepSelection`");
    s
}

fn obj(props: Value, required: &[&str]) -> Value {
    let mut o = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        o["required"] = json!(required);
    }
    o
}

/// A schema that also takes keys it doesn't list: [`refuse_unknown_args`] lets them through here.
trait OpenKeys {
    fn with_open_keys(self) -> Value;
}

impl OpenKeys for Value {
    fn with_open_keys(mut self) -> Value {
        self["additionalProperties"] = json!(true);
        self
    }
}

fn tool(name: &str, title: &str, desc: &str, schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": desc,
        "inputSchema": schema,
        "annotations": {"title": title, "readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false},
    })
}

/// Tool names, in `tools/list` order.
pub const TOOL_NAMES: &[&str] = &[
    "list_commands",
    "run_command",
    "batch",
    "create_node",
    "set_properties",
    "inspect_document",
    "get_node",
    "select",
    "get_selection",
    "find_nodes",
    "get_nodes",
    "contact_sheet",
    "export",
    "export_many",
    "export_source",
    "screenshot",
    "open_file",
    "save_file",
    "undo",
    "redo",
    "pointer",
    "key",
    "type_text",
    "inspect_ui",
    "get_metadata",
    "get_variable_defs",
    "get_design_context",
    "search_design_system",
    "lint_icons",
    "export_tokens",
    "check_contrast",
    "history",
    "auto_name_layers",
    "wait_for_changes",
    "diagnostics",
    "use_document",
    "copy_between_documents",
    "checkpoint",
    "list_versions",
    "restore_version",
    "compare_version",
    "find_similar",
    "propagate_change",
    "audit_tokens",
    "lint_layout",
    "get_code_connect_map",
    "add_code_connect_map",
    "diff_documents",
    "summarize_document",
    "check_accessibility",
];

/// The tools that take `documents`: one call per file (`"all"` or a list of ids), at most
/// [`MAX_DOCUMENTS`] files.
const MULTI_DOCUMENT_TOOLS: &[&str] = &["find_nodes", "set_properties", "run_command", "get_nodes"];

/// Most files one `documents` call works on.
const MAX_DOCUMENTS: usize = 100;

/// The tools that ignore the target document: they name their files themselves, or are not about
/// a document (`pointer`, `key` and `type_text` drive the tab in front, as the user would).
const UNTARGETED: &[&str] = &[
    "list_commands",
    "open_file",
    "use_document",
    "copy_between_documents",
    "diagnostics",
    "inspect_ui",
    "wait_for_changes",
    "pointer",
    "key",
    "type_text",
];

/// The tools that take `document`: they act on the file it names instead of the one in front.
const DOCUMENT_TOOLS: &[&str] = &[
    "run_command",
    "batch",
    "create_node",
    "set_properties",
    "inspect_document",
    "get_node",
    "select",
    "find_nodes",
    "get_nodes",
    "contact_sheet",
    "export",
    "screenshot",
    "save_file",
    "undo",
    "redo",
    "checkpoint",
    "list_versions",
    "restore_version",
    "compare_version",
    "find_similar",
    "propagate_change",
    "audit_tokens",
    "lint_layout",
    "get_code_connect_map",
    "add_code_connect_map",
    "diff_documents",
    "summarize_document",
    "check_accessibility",
];

/// All tools, in `tools/list` order.
pub fn tool_definitions() -> Vec<Value> {
    let mut tools = tools();
    let document = id_schema(
        "The open file to act on: its `id` (from run_command doc.list, or the `document` open_file and inspect_document return) or its path. Default: the target document (open_file, use_document), else the file in front. The tab in front stays in front; with an attached app, a screenshot of the window needs the file in front",
    );
    let documents = json!({
        "description": "Run this call once in each of these files and return {documents: [{document, name, ok, result | error}], totalMatched?} (find_nodes also `total`): \"all\" (every open file) or a list of document ids (from doc.list), at most 100. A file where the call fails does not stop the others; for set_properties a file where `where` matches nothing reports matched 0. With `where` this changes or finds the same element in every screen file at once. Not together with `document`",
        "anyOf": [{"type": "string", "enum": ["all"]}, {"type": "array", "maxItems": MAX_DOCUMENTS, "items": {"anyOf": [{"type": "integer"}, {"type": "string"}]}}],
    });
    for t in &mut tools {
        let Some(name) = t.get("name").and_then(Value::as_str).map(str::to_string) else { continue };
        let Some(props) = t.pointer_mut("/inputSchema/properties").and_then(Value::as_object_mut) else { continue };
        if DOCUMENT_TOOLS.contains(&name.as_str()) {
            props.insert("document".into(), document.clone());
        }
        if MULTI_DOCUMENT_TOOLS.contains(&name.as_str()) {
            props.insert("documents".into(), documents.clone());
        }
    }
    tools
}

fn tools() -> Vec<Value> {
    let empty = || obj(json!({}), &[]);
    vec![
        tool(
            "list_commands",
            "List commands",
            "List FigCraft commands: id, label, menu path, shortcut, whether it is undoable, and a parameter description. Every menu item, panel action and shortcut is a command; run one with run_command (or several as one undo step with batch). Most commands act on `ids` when given, else on the selection. Use `filter` to narrow (e.g. \"align\", \"layout\", \"variable\", \"style\", \"component\", \"proto\").",
            obj(
                json!({
                    "filter": string("Case-insensitive substring matched against id, label, menu path and parameter docs"),
                    "limit": int("Most commands to list (default 400, up to 2000)"),
                    "offset": int("Skip this many matching commands, to read the next page")
                }),
                &[],
            ),
            true,
        ),
        tool(
            "run_command",
            "Run command",
            "Execute one FigCraft command by id with JSON params (ids and params from list_commands). Mutating commands are one undo step each; a failing command leaves the document unchanged. Examples: {\"command\":\"align.left\",\"params\":{\"ids\":[3,4]}}, {\"command\":\"layout.add\",\"params\":{\"ids\":[2],\"direction\":\"vertical\"}}, {\"command\":\"style.create\",\"params\":{\"type\":\"paint\",\"name\":\"Brand/Primary\",\"from\":5}}. Returns the command's result (creation commands return {\"id\"}). Every command also takes the generic params `where` (node.find filters: act on the matching layers), `list` (act on a saved list), `dryRun` (change nothing, return {dryRun, wouldChange, created, deleted, count, result}) and `keepSelection` (leave the user's selection and view alone); list.save / list.get / list.delete / list.all manage the lists. Programmatic calls never open dialogs.",
            obj(
                json!({"command": string("Command id, e.g. node.create, align.left, component.create, file.export"), "params": command_params_schema()}),
                &["command"],
            ),
            false,
        ),
        tool(
            "batch",
            "Run commands as one step",
            "Run several commands in order as ONE undo step (engine `command.batch`). By default (onError: \"abort\"), if any command fails, everything is rolled back and the error names the failing step; success returns an array of the results in order. With onError: \"continue\", a failed step is rolled back on its own, the later steps still run, the call is not an error, and the array instead holds one {step, command, ok, result} or {step, command, ok, error} object per step; a reference to a failed step fails that step too. A later step can use what an earlier step returned: name a step with \"as\": \"card\", then anywhere in a later step's params {\"$ref\": \"card\"} stands for the `id` in its result ({\"$step\": n} does the same by zero-based position). Add \"field\" for another part of the result: a key or a slash path, e.g. {\"$ref\": \"copy\", \"field\": \"ids/0\"} for the first id edit.duplicate returned (with \"map\": true it also returns `map`, each original id → its copy, so \"field\": \"map/3\" is the copy of layer 3). Any object with a `$ref` or `$step` key is a reference. Example: [{\"command\":\"node.create\",\"as\":\"card\",\"params\":{\"type\":\"frame\"}}, {\"command\":\"node.create\",\"params\":{\"type\":\"text\",\"text\":\"Hi\",\"parent\":{\"$ref\":\"card\"}}}] builds a frame and a text inside it in one call.",
            obj(
                json!({
                    "commands": {"type": "array", "minItems": 1, "description": "Steps to run", "items": obj(json!({"command": string("Command id"), "params": command_params_schema(), "as": string("Name for this step, unique in the batch; later steps use its result with {\"$ref\": name}")}), &["command"])},
                    "label": string("Undo history label (default \"Batch\")"),
                    "quiet": boolean("Terse result: {ok, count, failures?} with only the failed steps (use with onError continue); step results are left out"),
                    "onError": {"type": "string", "enum": ["abort", "continue"], "description": "abort (default) rolls back the whole batch on the first failure; continue rolls back only the failed step, runs the rest and returns one {step, command, ok, result|error} object per step"},
                }),
                &["commands"],
            ),
            false,
        ),
        tool(
            "create_node",
            "Create layer",
            "Create a layer (engine `node.create`) and return its id and inspected properties. Coordinates are page units, y down; x/y are relative to the parent frame (the page when there is none, so then they are page coordinates; groups don't count). Inside an auto-layout frame the position is decided by the layout. The new layer becomes the selection.",
            obj(
                json!({
                    "type": {"type": "string", "enum": ["frame", "rectangle", "ellipse", "polygon", "star", "line", "arrow", "text", "vector", "section", "component", "slice"], "description": "Layer type"},
                    "x": num("Left, relative to the parent frame (default 0). For a vector with `path` or `points`, x/y move their own origin instead: data whose box starts at (30, 0) makes a layer whose left is x + 30. Start the path at 0,0 to place its left edge at x, or set x afterwards with set_properties (which moves the left edge)"),
                    "y": num("Top, relative to the parent frame (default 0). For a vector with `path` or `points`, their own top is added, as for x"),
                    "width": num("Width (default 100; text hugs its content unless width is set, which makes it wrap; for a vector with path, width/height scale the path, one alone keeping its proportion, and without them the path's own box is used)"),
                    "height": num("Height (default 100; ignored for lines/arrows and auto-height text)"),
                    "parent": id_schema("Parent frame/group/component id (default: the current page)"),
                    "index": int("Z-index among the parent's children (default: on top)"),
                    "name": string("Layer name"),
                    "key": string("Stable key unique in the document (e.g. \"icon/folder\"); survives save, open, undo and .fig export. Any command taking a layer id also takes {\"key\": \"icon/folder\"} there. A duplicate key is an error that suggests a free one."),
                    "text": string("Characters, for type text"),
                    "path": string("SVG path data, for type vector (e.g. \"M0 0 L100 0 L50 80 Z\"); its coordinates are kept relative to x/y, so a path that doesn't start at 0,0 is offset by its own box"),
                    "absolute": boolean("In an auto-layout parent: place the layer outside the flow at x/y (absolute positioning)"),
                    "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "For type vector, instead of path: the points [[x, y], ...]"},
                    "closed": boolean("For type vector: close the path"),
                    "select": boolean("Select the new layer (default true)"),
                    "dryRun": boolean("Create nothing; return {dryRun, wouldChange, created, deleted, count, result}"),
                    "keepSelection": boolean("Leave the user's selection and view as they are (the new layer is not selected)"),
                    "props": {"type": "object", "description": "Any set_properties keys applied at creation (they may also be given beside `type`, like fill or layout), e.g. {\"fill\":\"#3366FF\",\"cornerRadius\":12,\"fontSize\":24,\"layout\":\"vertical\",\"gap\":8,\"padding\":16}"},
                }),
                &["type"],
            )
            .with_open_keys(),
            false,
        ),
        tool(
            "set_properties",
            "Set properties",
            "Set properties on one or more layers as one undo step (engine `node.set`). Keys: name, visible, locked, opacity (0-1), blend, x, y (relative to the parent frame; groups don't count), width, height, rotation (degrees), fill (colour) / fills (array of paints: \"#hex\" or {\"type\":\"solid\",\"color\":\"#hex\"} or {\"type\":\"linearGradient\",\"stops\":[\"#hex\",\"#hex\"]} or stops [{\"position\":0..1,\"color\":\"#hex\"}], optional from/to handles in the unit box; radialGradient too), stroke / strokes, strokeWeight, strokeAlign (inside|center|outside), cornerRadius (n or [tl,tr,br,bl]), effects (array, e.g. {\"type\":\"dropShadow\",\"color\":\"#00000040\",\"offset\":[0,4],\"radius\":8}), clip, layout (null|horizontal|vertical|grid), gap, padding (n or [t,r,b,l]), mainAlign (min|center|max|spaceBetween|spaceAround|spaceEvenly), crossAlign, wrap, contentAlign (auto|spaceBetween: wrapped lines), sizingH / sizingV (fixed|hug|fill), constraints {horizontal, vertical}, characters, fontFamily, fontStyle, fontSize, lineHeight, letterSpacing, textAlign, textAutoResize. Auto layout keys (gap, padding, mainAlign, crossAlign, wrap, …) apply to frames, components and instances only: other layers among the ids are skipped and named in `warnings`, and with none that can take them the call is an error. A fontStyle draws in the family's nearest face; `warnings` says when that face has another weight or slant than the name asks for. Run list_commands with filter \"node.set\" for the full list. Unknown keys are an error. Returns the updated layers. Instead of `ids`, `where` (the node.find filters) picks the layers: they are found first, then changed together as one undo step, and the result carries `matched` and `ids`; no match is an error and nothing changes. `list` names a saved list of layers (list.save) instead; `dryRun` returns what would change without changing anything; `keepSelection` leaves the user's selection and view alone.",
            obj(
                json!({
                    "ids": ids_schema("Layer ids (default: the selection)"),
                    "where": where_schema(),
                    "list": string("Instead of `ids`: the layers of this saved list (list.save)"),
                    "props": {"type": "object", "description": "Property values to set"},
                    "quiet": boolean("Return only {ok, ids, warnings?} instead of every updated layer (default true for more than one layer)"),
                    "dryRun": boolean("Change nothing; return {dryRun, wouldChange, created, deleted, count, result}"),
                    "keepSelection": boolean("Leave the user's selection and view as they are"),
                }),
                &["props"],
            ),
            false,
        ),
        tool(
            "inspect_document",
            "Inspect document",
            "The active document as JSON: name, path, dirty flag, current page, selection, the current page's layer tree (id, name, type, x/y relative to the parent frame (groups don't count), width, height, absoluteBounds in page coordinates, fills, strokes, effects, cornerRadius, layout, sizing, text properties, component links, overrides, bound variables), other pages summarised, styles, variable count and the undo history. Use it to verify every change. Each level of `depth` multiplies the response, and `allPages` multiplies it again: on a large document the tree is reported at a shallower depth than asked for, with a `truncated` object saying so. Use get_node to read one subtree deeply instead.",
            obj(
                json!({
                    "depth": int("Tree depth for the current page (default 4)"),
                    "fields": fields_schema(),
                    "outline": outline_schema(),
                    "allPages": boolean("Expand every page, not only the current one"),
                    "maxBytes": int(
                        "Largest response to send, in bytes of the whole JSON-RPC message (default 8388608, which fits the usual 10 MiB client buffer); 1024 of it is kept for the envelope and the request id"
                    ),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "get_node",
            "Get layer",
            "One layer as JSON (same fields as inspect_document) with its children to `depth`. A subtree too large for one message is reported at a shallower depth, or with a window of the children, and a `truncated` object says so and gives the `offset` to ask for next.",
            obj(
                json!({
                    "id": id_schema("Layer id"),
                    "key": string("The layer's key (e.g. \"icon/folder\") instead of `id`"),
                    "depth": int("Child depth (default 1)"),
                    "fields": fields_schema(),
                    "outline": outline_schema(),
                    "offset": int("First child to report (default 0), for reading a node with more children than fit in one message"),
                    "maxBytes": int(
                        "Largest response to send, in bytes of the whole JSON-RPC message (default 8388608, which fits the usual 10 MiB client buffer); 1024 of it is kept for the envelope and the request id"
                    ),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "select",
            "Select layers",
            "Replace the selection with `ids` (switching to their page). An empty array deselects everything. Most commands act on the selection when no `ids` are passed. Instead of `ids`, `where` (the node.find filters, e.g. {\"type\": \"TEXT\", \"nameGlob\": \"Label*\"}) selects every match and returns `matched` and `ids`; when nothing matches the selection is left as it is. `list` selects a saved list of layers (list.save); with `dryRun` nothing is selected and the result only reports the matches. This replaces the user's selection: to act on layers without touching it, pass `where`, `list` or `ids` to the tool that does the work, with `keepSelection`.",
            obj(
                json!({
                    "ids": ids_schema("Layer ids to select"),
                    "where": where_schema(),
                    "list": string("Instead of `ids`: select the layers of this saved list (list.save)"),
                    "dryRun": boolean("Change nothing; the result lists the layers that would be selected as `wouldSelect`"),
                    "keepSelection": boolean("Accepted for symmetry with the other tools; selecting always replaces the selection"),
                }),
                &[],
            ),
            false,
        ),
        tool(
            "get_selection",
            "Get selection",
            "What the user has selected right now in the open document: {page: {id, name}, selection: [{id, name, type, parent, x, y, width, height}], count}. x/y are relative to the parent frame (groups don't count). An empty selection gives count 0. Call it first when the user says \"this\", \"it\" or \"the selected layer\".",
            empty(),
            true,
        ),
        tool(
            "find_nodes",
            "Find layers",
            "Search for layers with the node.find filters and return {ids, total} without selecting anything or changing the page, view or selection (read-only; safe on the user's open file). All filters are ANDed; text filters ignore case. `page` (an id, a name or \"all\") searches another page without switching to it. `details: true` adds `nodes`: [{id, name, type, parent, x, y, width, height}] (x/y relative to the parent), so one call finds and describes. At most `limit` layers are returned (default 100, up to 10000); `total` counts every match. Combine with get_nodes (outline), contact_sheet (pictures) or the `where`/`list` of the tools that change layers.",
            obj(find_props(), &[]),
            true,
        ),
        tool(
            "get_nodes",
            "Get many layers",
            "A cheap outline of many layers in one call, without selecting them: for each layer the chosen `fields` plus `childCount`, and its children down to `depth` with the same fields. Name the layers with `ids`, `where` (the node.find filters) or `list` (a saved list), exactly one of them. Returns {count, nodes: [{id, ...fields, childCount, children?}], total?, truncated?}; at most `limit` layers (default and maximum 200) and a response that fits one message: `truncated` says what was left out. A layer that cannot be read is {id, error}. Use get_node for one layer in depth.",
            obj(
                json!({
                    "ids": ids_schema("Layer ids"),
                    "where": where_schema(),
                    "list": string("A saved list of layers (list.save)"),
                    "depth": json!({"type": "integer", "minimum": 0, "maximum": 6, "description": "Child levels below each layer (default 1, max 6; 0 = the layer and its childCount only)"}),
                    "fields": nodes_fields_schema(),
                    "limit": json!({"type": "integer", "minimum": 1, "maximum": 200, "description": "Most layers to read (default and maximum 200)"}),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "contact_sheet",
            "Contact sheet",
            "One PNG with many frames side by side to compare them at a glance: each layer rendered on its own, laid out in a grid on a dark background, with its index number (1, 2, 3 ...) in a box at its corner. Returns the image and a JSON legend {frames: [{index, id, name, width, height}], columns, scale, width, height, truncated?, failed?}. Name the layers with `ids`, `where` (the node.find filters) or `list` (a saved list), exactly one of them; at most 64 are drawn (`truncated` says how many were left out). Read-only: nothing is selected and the view does not move. Use screenshot for one layer at full size.",
            obj(
                json!({
                    "ids": ids_schema("Layers to draw, in this order"),
                    "where": where_schema(),
                    "list": string("A saved list of layers (list.save)"),
                    "scale": json!({"type": "number", "minimum": 0.02, "maximum": 1, "description": "Pixel scale of each frame (default 0.25); smaller when the sheet would pass maxSize"}),
                    "columns": json!({"type": "integer", "minimum": 1, "maximum": 16, "description": "Frames per row (default 4)"}),
                    "maxSize": json!({"type": "integer", "minimum": 64, "maximum": 8192, "description": "Longest side of the sheet in pixels (default 2400)"}),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "export",
            "Export",
            "Export layers (default: the selection, else every top-level layer of the current page) as png, jpg, svg, webp, pdf or xml (an Android Vector Drawable, with `warnings` for what it changed or left out, per `vdCompat`) at `scale`, or the whole file as `fig` (experimental: written from public format knowledge, with `warnings` for what was left out; other apps may not open every file) (PDF: with `bleed`, `cropMarks`, `bleedMarks` and `cmyk` for print; options `contentsOnly`, `includeBoundingBox`, and for SVG `svgOutlineText`, `svgIdAttribute`, `svgSimplifyStroke`; for XML `svgIdAttribute` and `vdCompat`). With `path` the file is written there (absolute path recommended) and the result says where. Without `path`: png/jpg/webp come back as an image content block, svg as text, xml as text followed by `{format, warnings}`, other formats as base64 `data`.",
            obj(
                json!({
                    "ids": ids_schema("Layers to export (several are composited into one image)"),
                    "format": {"type": "string", "enum": ["png", "jpg", "svg", "webp", "pdf", "xml", "fig"], "description": "Format (default png, or from the path's extension)"},
                    "scale": num("Pixel scale for raster formats (default 1, max 64)"),
                    "size": string("Instead of `scale`: `2x`, or a width or height in pixels like `512w` / `512h`"),
                    "path": string("Output file path"),
                    "bleed": {"type": ["number", "string"], "description": "PDF: bleed around each page, in points or as a length like `3mm` or `0.125in`"},
                    "cropMarks": boolean("PDF: crop marks"),
                    "bleedMarks": boolean("PDF: bleed marks"),
                    "cmyk": boolean("PDF: CMYK colour instead of RGB"),
                    "contentsOnly": boolean("Only the layers themselves (default true); false also draws every other layer that shows in their area"),
                    "includeBoundingBox": boolean("Export the layers' own boxes, cutting off shadows and outside strokes (default false)"),
                    "svgOutlineText": boolean("SVG: text as outlines (default true); false keeps `<text>` elements"),
                    "svgIdAttribute": boolean("SVG: layer names as unique `id` attributes (default false)"),
                    "svgSimplifyStroke": boolean("SVG: strokes as `stroke` attributes where exact (default true); false writes outlines"),
                    "vdCompat": {"type": "string", "enum": ["bestEffort", "exact", "ignore"], "description": "XML: what a Vector Drawable can't draw exactly is approximated with a warning (bestEffort, default), left out with a warning (exact), or written as far as it maps without warnings (ignore: overlapping layer opacity dropped, even-odd fills kept as fillType=evenOdd, diamond gradients left out)"},
                    "cvd": {"type": "string", "enum": ["protanopia", "deuteranopia", "tritanopia", "achromatopsia"], "description": "PNG only: simulate this colour vision deficiency"},
                    "iconPreset": boolean("SVG: tidy for icons (transforms baked into the paths, no needless groups or view-box clips, numbers to 3 decimals, the layers' box as the view box)"),
                    "currentColor": {"type": ["boolean", "string"], "description": "With iconPreset: true makes the icon's only colour `currentColor`; a hex colour like \"#111111\" does so for that colour"},
                }),
                &[],
            ),
            true,
        ),
        tool(
            "export_many",
            "Export many",
            "Export each layer (default: the selection, else every top-level layer of the page) on its own in every format and scale of `formats` x `scales`, without export settings on the layers (engine `file.exportMany`): 6 icons x png/svg x 1/2/4x is one call. Names follow `naming` (`{name}`, `{id}`, `{scale}`, `{format}`, `{ext}`; default `{name}@{scale}x.{ext}` for raster with several scales, else `{name}.{ext}`; `/` in a name makes folders; repeated names get ` 2`). With `dir` the files are written there and the result lists `path`s; without it the files come back base64-encoded in `data` ({name: base64}). `iconPreset` (with optional `currentColor`) tidies SVGs for icons. Returns `{files: [{name, id, format, scale, bytes, path?}], data?}`.",
            obj(
                json!({
                    "ids": ids_schema("Layers to export, each on its own (default: the selection, else the page's top-level layers)"),
                    "formats": {"type": "array", "items": {"type": "string", "enum": ["png", "jpg", "webp", "svg", "pdf", "xml"]}, "description": "Formats (default [\"png\"])"},
                    "scales": {"type": "array", "items": {"type": "number"}, "description": "Pixel scales for raster formats (default [1]; svg, pdf and xml ignore them)"},
                    "naming": string("File name template, e.g. \"{name}@{scale}x.{ext}\""),
                    "dir": string("Folder to write into (absolute path recommended); omit to get base64 `data`"),
                    "iconPreset": boolean("SVG: tidy for icons (baked transforms, no needless groups or clips, 3 decimals, the layer's box as view box)"),
                    "currentColor": {"type": ["boolean", "string"], "description": "With iconPreset: true = the icon's only colour, or a hex colour, becomes `currentColor`"},
                    "svgOutlineText": boolean("SVG: text as outlines (default true)"),
                    "svgIdAttribute": boolean("SVG: layer names as `id` attributes"),
                    "svgSimplifyStroke": boolean("SVG: strokes as `stroke` attributes where exact (default true)"),
                    "contentsOnly": boolean("Only the layers themselves (default true)"),
                    "includeBoundingBox": boolean("Export the layers' own boxes (default false)"),
                }),
                &[],
            ),
            false,
        ),
        tool(
            "export_source",
            "Export plain-text source",
            "The whole document as canonical plain text for diffs and version control (engine `file.exportSource`; format in docs/source-format.md): JSON Lines, one object per line with sorted keys and no volatile fields, so exporting twice gives identical text and changing one fill changes one line. Layers keep their ids and keys. Without `path` the text comes back as `source`; with a `path` that has a file extension it is written there, and with a folder path (no extension) `design.jsonl` and `images/<hash>` are written. Read it back with run_command file.importSource.",
            obj(json!({"path": string("Output file (with extension) or folder (without); omit to get the text back")}), &[]),
            true,
        ),
        tool(
            "screenshot",
            "Screenshot",
            "Look at the design: renders the given layers (or the whole current page, with its background) to PNG and returns it as image content, plus its size and the scale used. The image is scaled down to fit `maxSize` pixels. With `window: true` (or a window id from `inspect_ui`'s `windows`) and an attached app, captures that app window including the UI instead.",
            obj(
                json!({
                    "ids": ids_schema("Layers to render (default: the whole current page)"),
                    "scale": num("Requested scale (default 1; reduced to fit maxSize)"),
                    "maxSize": int("Largest side in pixels (default 1600)"),
                    "cvd": {"type": "string", "enum": ["protanopia", "deuteranopia", "tritanopia", "achromatopsia"], "description": "Show the image as a person with this colour vision deficiency sees it (Machado 2009 matrices)"},
                    "window": {"description": "Capture the app window: true for the one in front, or a window id (attached app only; headless renders the page)", "anyOf": [{"type": "boolean"}, {"type": "integer"}]},
                    "marks": {"description": "Set-of-marks for vision models: true, or {ids?: layers to mark (default the rendered layers, or the page's top-level layers), style?: \"index\" (badges 1, 2, 3 ...; default) or \"id\" (the layer id), limit?: most badges (default 60, up to 200)}. A white-on-dark badge at the top left of each marked layer and of its children down to `markDepth`; the result's `marks` maps badge to layer id and name. Not with `window`", "anyOf": [{"type": "boolean"}, {"type": "object"}]},
                    "markDepth": int("With `marks`: levels of children below each marked layer that get a badge too (default 1; 0 = only the layers themselves; up to 5)"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "open_file",
            "Open file",
            "Open a .figcraft or .json document (FigCraft or REST API file JSON), a .fig or .sketch file (best effort, with `warnings` for what wasn't imported; .sketch from Sketch 48 or newer), an .svg (becomes vectors) or an image (becomes an image layer). Headless, the file becomes the active document. With an attached app it opens in a background tab and the user's tab and view stay as they are: the file becomes the target document (later calls act on it; use_document or `document` name another); `focus: true` also switches the user's window to it. Returns the new document's summary and its `document` id.",
            obj(
                json!({
                    "path": string("File path"),
                    "focus": boolean("Attached app: switch the user's window to the opened file (default false: it opens in the background)"),
                    "variables": {"description": "REST API file JSON only: the local variables JSON (`GET /v1/files/:key/variables/local`) as a path or the JSON object. Default: `<name>.variables.json` or `variables.json` next to the file."},
                    "reload": boolean("A file that is already open: read it again from disk into its tab (refused while the tab has unsaved changes). Without it, the result says `stale: true` when the file changed on disk since the tab read or saved it"),
                }),
                &["path"],
            ),
            false,
        ),
        tool(
            "save_file",
            "Save file",
            "Save the active document as a .figcraft file. `path` is required the first time (an untitled document has no path); later saves reuse it. A `path` ending in .fig writes an experimental .fig export copy instead and leaves the document's own path and unsaved changes alone. With an attached app, a result with `\"unsavedBefore\": true` says the window had unsaved changes before the call (the person's too: they are written); `\"changedOnDisk\": true` says the file on disk differed from what the document last read or saved.",
            obj(json!({"path": string("Output .figcraft path, or a .fig path for an experimental export copy")}), &[]),
            false,
        ),
        tool("undo", "Undo", "Undo the last change (one command, batch or gesture). Returns {undone: bool}.", empty(), false),
        tool("redo", "Redo", "Redo the last undone change. Returns {redone: bool}.", empty(), false),
        tool(
            "pointer",
            "Pointer gesture",
            "Drive the canvas with the mouse in page coordinates, exactly as a user would with the current tool: click to select, drag to move/resize/marquee, or draw with a shape tool (set `tool` first, e.g. {\"tool\":\"rectangle\",\"kind\":\"drag\",\"x\":0,\"y\":0,\"to\":[200,100]}). `double` on a text layer starts editing it. Returns the selection and tool.",
            obj(
                json!({
                    "kind": {"type": "string", "enum": ["click", "double", "drag", "down", "move", "up", "hover"], "description": "Gesture (default click)"},
                    "x": num("Page x"),
                    "y": num("Page y"),
                    "to": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "Drag end [x, y] (page)"},
                    "steps": int("Intermediate move events for drag (default 8)"),
                    "tool": {"type": "string", "description": "Tool to choose first: move|scale|hand|frame|section|slice|rectangle|line|arrow|ellipse|polygon|star|pen|pencil|text|comment"},
                    "shift": boolean("Shift held"),
                    "alt": boolean("Alt/Option held"),
                    "cmd": boolean("Cmd (macOS) / Ctrl held"),
                    "ctrl": boolean("Ctrl held (the Control key on macOS)"),
                    "pressure": num("Pen pressure 0-1 (the Pencil records it as the stroke's width profile; a drag starts with it)"),
                    "pressureTo": num("Pen pressure 0-1 at a drag's end (interpolated along it)"),
                }),
                &["x", "y"],
            ),
            false,
        ),
        tool(
            "key",
            "Press key",
            "Press a key or shortcut as if typed on the canvas: a menu shortcut (e.g. {\"key\":\"G\",\"cmd\":true} groups, {\"key\":\"A\",\"alt\":true,\"shift\":false} …), a tool key (R, O, T, F, V…), Escape, Delete, Enter, arrows (nudge; shift = big nudge). While editing text, keys go to the text editor (Backspace, Delete, Left, Right, Home, End, Enter, Escape, SelectAll).",
            obj(
                json!({"key": string("Key name: a letter/digit, Escape, Delete, Backspace, Enter, ArrowLeft…"), "shift": boolean("Shift"), "alt": boolean("Alt/Option"), "cmd": boolean("Cmd/Ctrl"), "ctrl": boolean("Ctrl (the Control key on macOS)")}),
                &["key"],
            ),
            false,
        ),
        tool(
            "type_text",
            "Type text",
            "Type into a text layer. With `id`, starts editing that layer with all of its text selected (so the typed text replaces it), types, then stops editing. Without `id`, types at the caret of the text currently being edited (after a double-click with pointer, or after drawing with the text tool).",
            obj(json!({"text": string("Text to type (\\n for a new line)"), "id": id_schema("Text layer to replace the contents of")}), &["text"]),
            false,
        ),
        tool(
            "inspect_ui",
            "Inspect UI",
            "UI state: current tool, view (zoom, centre), status, panels, timings and the app's windows (`windows`: id, title, files) when attached to the app; headless reports the session status.",
            empty(),
            true,
        ),
        tool(
            "get_metadata",
            "Get layer outline",
            "A sparse XML outline of a layer and its descendants (engine `inspect.metadata`): one element per layer with id, name, x/y (relative to the parent), width and height; `hidden` for invisible layers and `omitted` = how many children are not shown (depth or node limit). Use it to find ids, then get_design_context or get_node for details. Same name and arguments as the Figma MCP tool.",
            obj(
                json!({
                    "nodeId": id_schema("Layer id (default: the current page)"),
                    "depth": int("Levels below the layer to list (default 4, max 32)"),
                    "maxNodes": int("Most layers listed (default 500, max 5000)"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "get_variable_defs",
            "Get variables used",
            "The variables (design tokens) a layer, its descendants and their styles use (engine `inspect.variableDefs`): {name: resolved value} in the layer's variable modes; colours as hex. Same name and arguments as the Figma MCP tool.",
            obj(json!({"nodeId": id_schema("Layer id (default: the selection)")}), &[]),
            true,
        ),
        tool(
            "get_design_context",
            "Get design context",
            "Everything needed to implement a layer in code (engine `inspect.designContext`): generated `code` (HTML + CSS by default; `language` as inspect.code: css, html, react, vue, tailwind, swiftui, compose…), the layer's `properties` (layout, typography, fills, strokes, effects with style and variable names), `variables` used, exportable `assets` and a child `outline`. `include` adds more for the layer and its descendants: `instances` (with Code Connect snippets, which also replace generated markup in `code`), `annotations`, `devResources`, `status`, `comments` and `interactions` (both opt-in), `bindings` (variable-bound properties as var(--name, raw value)) and `hints` (use mapped components first, then tokens, then raw values). Same name and arguments as the Figma MCP tool.",
            obj(
                json!({
                    "nodeId": id_schema("Layer id (default: the selection)"),
                    "language": string("css | html | react | leptos | vue | tailwind | swiftui | compose | arkui (default html)"),
                    "depth": int("Outline depth (default 3, max 32)"),
                    "include": {"type": "array", "items": {"type": "string", "enum": ["instances", "annotations", "devResources", "status", "comments", "bindings", "interactions", "hints", "all"]}, "description": "Extras to add (default all but comments and interactions)"},
                }),
                &[],
            ),
            true,
        ),
        tool(
            "search_design_system",
            "Search design system",
            "Search the file's components, variables and styles by name or description (engine `inspect.searchDesignSystem`). Returns `{components, variables, styles, total}`; components are ids to place with `instance.place`, variables and styles to apply with `variable.bind` / `style.apply`. Same name as the Figma MCP tool.",
            obj(
                json!({
                    "query": string("Case-insensitive text to find in names and descriptions; empty lists everything"),
                    "types": {"type": "array", "items": {"type": "string", "enum": ["component", "variable", "style"]}, "description": "Kinds to search (default all)"},
                    "limit": int("Most results per kind (default 25, max 200)"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "lint_icons",
            "Lint icon set",
            "Check an icon set for inconsistencies (engine `inspect.lintIcons`): stroke width, cap and join, corner radius, colour use, pixel-grid alignment (edges on whole pixels at the grid size; half pixels for odd strokes), padding inside the live area, icon size and naming. `ids` is the icon-set frame (its small frames/groups/shapes are the icons) or icons themselves. Expected values default to the most common ones in the set; pass `strokeWeight`, `cap`, `join`, `color` to set them. Returns `{icons, grid, expected, issues: [{rule, severity, id, name, message, found, expected, fixable, fix?: {command, params}}], total, truncated, fixable, counts}`. Read-only: run a `fix` with run_command or batch.",
            obj(
                json!({
                    "ids": ids_schema("The icon-set frame, or icons (default: the selection)"),
                    "grid": num("Icon size in pixels (default 24)"),
                    "strokeWeight": num("Stroke width every icon should use (default: the most common)"),
                    "cap": string("Stroke cap every icon should use, e.g. round (default: the most common)"),
                    "join": string("Stroke join every icon should use, e.g. round (default: the most common)"),
                    "color": string("Colour every icon should use, e.g. #111111 (default: the most common)"),
                    "padding": num("Free space kept at each edge, strokes included (default grid/12, 2 at 24)"),
                    "tolerance": num("Allowed distance from the pixel grid (default 0.02)"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "export_tokens",
            "Export design tokens",
            "Export the file's variables as design tokens (engine `variable.exportTokens`). `format` dtcg (default): Design Tokens Format Module files, one per collection and mode, `{files: [{name, collection, mode, tokens}]}`, colours as `{colorSpace: \"srgb\", components, alpha?, hex}`, lengths as `{value, unit: \"px\"}`, aliases as `{group.token}`. `css`: custom properties, the default mode on `:root` and the others on `[data-theme=\"<mode>\"]`. `tailwind`: `{theme: {extend: {colors, borderRadius, fontSize, spacing, fontFamily}}, text}` with `var(--name)` values and the css that defines them. `json`: the local variables JSON. `dtcg-styles`: text styles as typography, effect styles as shadow, paint styles as color, gradient or border composite tokens with `com.figcraft` $extensions. `style-dictionary`: a v4 scaffold, `{files: [{name, text | tokens}]}` with a config and a token file (hex colours, `16px`) per collection and mode, plus the styles. `resolver`: a DTCG resolver document (modes as modifier contexts) with the token files. `swift`, `kotlin`: constants as text (Color/CGFloat, Compose Color/dp), the default mode under the variables' iOS / ANDROID code syntax. `android`: `values/colors.xml`, `values/dimens.xml`, `values-night/colors.xml`. Read-only unless `path` writes a file (a folder for the multi-file formats). Import DTCG back with run_command `variable.importTokens`.",
            obj(
                json!({
                    "format": {"type": "string", "enum": ["dtcg", "dtcg-styles", "style-dictionary", "resolver", "css", "tailwind", "json", "swift", "kotlin", "android"], "description": "Output format (default dtcg)"},
                    "collection": string("Collection id or name (default every collection of the file)"),
                    "modes": {"type": "array", "items": {"type": "string"}, "description": "Mode names or ids to include (default all)"},
                    "path": string("Write to this file (multi-file formats: this folder)"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "check_contrast",
            "Check contrast",
            "Check text contrast against what is painted behind it (engine `a11y.checkContrast`): WCAG 2.x ratio against AA/AAA for the text size and weight, and APCA Lc, per variable mode when colours are bound to variables. Returns `{level, checked, failures: [{id, name, mode, fg, bg, ratio, requiredRatio, apcaLc, fontSize, large, boundVariable?, fixable, suggestion: {textColor, ratio}}], unknown: [{id, reason}], passed}`; `unknown` is text over a gradient, image or blended fill. With `fg` and `bg` it checks one colour pair instead (`a11y.contrast`: #767676 on #FFFFFF is 4.54). Read-only.",
            obj(
                json!({
                    "ids": ids_schema("Layers whose text is checked (default: the whole current page)"),
                    "level": {"type": "string", "enum": ["AA", "AAA"], "description": "WCAG level (default AA)"},
                    "all": boolean("Also list the passing text layers as `results`"),
                    "fg": string("A colour pair instead of layers: text colour"),
                    "bg": string("A colour pair instead of layers: background colour"),
                    "fontSize": num("With fg and bg: text size in px (default 16)"),
                    "bold": boolean("With fg and bg: bold text"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "history",
            "Undo history",
            "The active document's undo stack as data (engine `history.list`, `history.revertTo`, `history.hash`). Default: list the newest `limit` steps, oldest first: `{entries: [{index, label, changedIds, changedCount, time, kind, applied}], applied, total, hash}`; a `batch` step carries its `label`. `revertTo` = an entry `index` undoes (or redoes) back to the state just before that entry as one operation and returns the new `hash` (refused across a version restore). `hash: true` returns just `{hash}`, a content hash of the document, to verify that two states are equal.",
            obj(
                json!({
                    "limit": int("Most entries to list (default 20, max 200)"),
                    "revertTo": int("Go to the state just before this history index (`total` = the latest state)"),
                    "hash": boolean("Return only the document content hash"),
                }),
                &[],
            ),
            false,
        ),
        tool(
            "auto_name_layers",
            "Rename generic layers",
            "Rename generic layer names (Vector, Rectangle 12, Frame 5, Group 3, Text 2) from context with fixed rules (engine `layer.autoName`): icon-like vectors in an icon frame become `icon/<parent>`, a shape filling a button `button/bg`, other shapes `<parent>/<shape>`, a text layer its content, a frame or group holding one text layer that text. One undo step. Use `dryRun` to see `{renames: [{id, from, to}], count}` without changing anything.",
            obj(
                json!({
                    "ids": ids_schema("Layers whose subtrees are renamed (default: the selection)"),
                    "scope": string("selection | page | document (default selection; the current page when nothing is selected)"),
                    "dryRun": boolean("Only return the proposed renames"),
                }),
                &[],
            ),
            false,
        ),
        tool(
            "wait_for_changes",
            "Wait for changes",
            "Wait for new comment events in the attached FigCraft app. Returns pending events immediately; otherwise waits up to `timeoutMs` (default 20000, max 25000). Call it again with the returned `cursor` to keep watching. Headless returns immediately (no other writer). Events carry a sequence number, kind, document and a small payload (for comments, the thread id).",
            obj(
                json!({
                    "after": int("Return events after this cursor (default 0)"),
                    "timeoutMs": int("Longest wait in milliseconds (default 20000, max 25000)"),
                    "topics": {"type": "array", "items": {"type": "string", "enum": ["comments", "document", "all"]}, "description": "Event topics (default [\"comments\"]; document events are not emitted yet)"},
                    "limit": int("Most events to return (default 100, max 1000)")
                }),
                &[],
            ),
            true,
        ),
        tool(
            "diagnostics",
            "Diagnostics",
            "Which backend this server talks to and what it has open: {backend, attached, mcpServer: {pid}, documents: [{index, id, name, path, dirty, changedOnDisk, lastSaved, active}]}. `attached` is true when the FigCraft app is driven (edits show in its window), false for a detached headless copy. `changedOnDisk`: the file changed on disk since the document was read or saved. `lastSaved` is the file's modification time (ISO 8601 UTC), null without a file. `path` is null for an untitled document and for AI agents (the app keeps file locations from them). `target` is the document later calls act on (see use_document; null: the file in front) and `autoSave` whether the app saves files by itself. Use it when edits do not show up or a save is refused.",
            empty(),
            true,
        ),
        tool(
            "use_document",
            "Use document",
            "Set the target document: the file every later call without `document` acts on, so an agent keeps working on its own file while the user clicks through other tabs (open_file with an attached app sets it to the file it opened). `document` is an id (from doc.list), a path or a name; null clears the target and calls act on the file in front again. Returns {target, documents: [{index, id, name, dirty, active}]}. The target is kept by this server only; `pointer`, `key` and `type_text` always drive the file in front.",
            obj(
                json!({"document": {"description": "The open file to work on: its id, path or name; null for the file in front", "anyOf": [{"type": "integer"}, {"type": "string"}, {"type": "null"}]}}),
                &["document"],
            ),
            false,
        ),
        tool(
            "copy_between_documents",
            "Copy layers between files",
            "Copy layers from one open file into another (engine `edit.copy` in `from`, `edit.paste` in `to`) without switching either tab or changing either selection. Name the layers with `ids` or `where` (the node.find filters, run in `from`), exactly one of them. They are pasted at the top level of the page `to` shows, or into `parent` (a layer of `to`), in place or with their top-left at page coordinates `x`, `y` (both). Returns {copied, newIds, from, to}. The copy is not a move: the originals stay.",
            obj(
                json!({
                    "from": id_schema("The file to copy from: its id (doc.list) or path"),
                    "to": id_schema("The file to paste into: its id (doc.list) or path"),
                    "ids": ids_schema("Layers of `from` to copy"),
                    "where": where_schema(),
                    "parent": id_schema("A frame, group or section of `to` to paste into (default: its current page)"),
                    "x": num("Page x of the pasted layers' top-left in `to` (with y; default: where they were)"),
                    "y": num("Page y of the pasted layers' top-left in `to` (with x)"),
                }),
                &["from", "to"],
            ),
            false,
        ),
        tool(
            "checkpoint",
            "Save a checkpoint",
            "Save a named version of the document as it is now (engine `file.saveVersion`), a point in its history to come back to with restore_version; do it before a big change. It does not save the file to disk. Returns {id, name, time}. Without `name` the version is named by the engine.",
            obj(json!({"name": string("Version name"), "description": string("What state this is")}), &[]),
            false,
        ),
        tool(
            "list_versions",
            "List versions",
            "The document's saved versions, newest last (engine `version.list`): {total, versions: [{id, index, name, description, time (Unix seconds), auto?, previewing?}]}; `auto` marks the automatic checkpoints (named Autosave). Use an `id` with restore_version or compare_version.",
            obj(json!({"limit": int("Most versions to return, the newest ones (default 30, up to 100)")}), &[]),
            true,
        ),
        tool(
            "restore_version",
            "Restore a version",
            "Replace the document with a saved version (engine `version.restore`) as one undo step (undo brings the current state back). The engine first keeps the state being replaced as an Autosave version, unless a version of it was just saved or restored, so nothing is lost. Refused unless `confirm` is true.",
            obj(
                json!({"id": string("The version's id (list_versions)"), "confirm": boolean("Must be true: the document is replaced")}),
                &["id"],
            ),
            false,
        ),
        tool(
            "compare_version",
            "Compare with a version",
            "What changed since a saved version (engine `dev.compare` with `version`): for the top-level frame or section `node`, or without it for every top-level frame and section of the current page, the added, removed and changed layers with their property changes {id, name, type, change, properties: [{property, before, after}]}. Frames and sections deleted since the version are not listed (run_command version.preview shows a version). Read-only.",
            obj(
                json!({"id": string("The version's id (list_versions)"), "node": id_schema("A top-level frame or section (default: all of the current page)")}),
                &["id"],
            ),
            true,
        ),
        tool(
            "find_similar",
            "Find similar layers",
            "Layers that look like the same element as `id` on any page of the document (engine `family.find`): a header copied onto every screen, a repeated card. Scores layers by family tag, shared component, structure, name and size: {id, family, total, twins: [{id, name, type, page, frame, score, reasons, sameFrame}]}, best first. Use it after changing one layer to see where else the same change may belong, then propagate_change. `documents` is not supported; pass `document` and call it per file.",
            obj(
                json!({"id": id_schema("The layer (default: the selection)"), "minScore": json!({"type": "number", "description": "Lowest score to list, 0-1 (default 0.6)"}), "limit": int("Most twins to return (default 50, up to 1000)")}),
                &[],
            ),
            true,
        ),
        tool(
            "propagate_change",
            "Apply a change to similar layers",
            "Copy the look of the layer `from` onto its twins (engine `family.propagate`), so a change made on one screen reaches the same element on the others. Twins are the `ids` you pass, else the layers find_similar scores at `minScore` (default 0.8). By default it copies every look property that differs (fills, strokes, effects, corner radius, opacity, type style…) and never names, text, positions or sizes; `props` lists the exact property names instead (naming `characters` copies text). Children are paired by position when both layers have the same child types (`deep`, default true). Nothing is changed unless `confirm` is true: without it the call returns the preview (`dryRun`) of what would change. Applied, it is one undo step. Returns {from, twins, changed: [{id, props}], skipped: [{id, prop, why}]}.",
            obj(
                json!({
                    "from": id_schema("The layer that was changed"),
                    "ids": json!({"type": "array", "items": id_schema("A twin"), "description": "Twins to change (default: the layers find_similar finds)"}),
                    "props": json!({"type": "array", "items": string("Property name, as get_node shows it"), "description": "Only these properties"}),
                    "minScore": json!({"type": "number", "description": "Lowest twin score when `ids` is not given, 0-1 (default 0.8)"}),
                    "deep": boolean("Pair and copy children too (default true)"),
                    "confirm": boolean("true applies the change; without it the call only previews")
                }),
                &["from"],
            ),
            false,
        ),
        tool(
            "audit_tokens",
            "Find colours that should be variables",
            "Visible solid fills and strokes that carry a literal colour a colour variable already holds (engine `lint.tokens`), so the design follows its tokens: {total, listed, hardcoded: [{id, name, field, color, variables}], fixes: [{variable, field, ids}], bound}. Each entry of `fixes` (hits with exactly one matching variable) is one run_command `variable.bind` {ids, field, variable}. Limited to `root` (a layer) or `page` (id, name or \"all\"; default the open page).",
            obj(
                json!({"root": id_schema("Only this layer and what is inside it"), "page": string("A page id or name, or \"all\" (default: the open page)"), "limit": int("Hits to list (default 200, up to 5000)")}),
                &[],
            ),
            true,
        ),
        tool(
            "lint_layout",
            "Find layout defects",
            "Layout defects among sibling layers (engine `lint.layout`), so you can check a design without a screenshot: {total, listed, truncated?, findings: [{check, ids, message, fix?: {command, params}}]}. Checks: overlap (siblings that intersect, neither inside the other; auto-layout children and absolute-positioned layers are skipped), misaligned (edges or centres 0-4 px from a shared line), uneven-gap (a row or column of 3+ layers whose gaps differ by up to 4 px), tiny-target (button-like frames, components and instances, or layers with interactions, under 24x24, or 44x44 in frames up to 500 wide), off-grid (x/y not a multiple of `grid`; only when named). Run a finding's `fix` with run_command (align.*, distribute.*, node.set). Limited to `root` (a layer) or `page` (id, name or \"all\"; default the open page).",
            obj(
                json!({"root": id_schema("Only this layer and what is inside it"), "page": string("A page id or name, or \"all\" (default: the open page)"), "checks": json!({"type": "array", "items": {"type": "string", "enum": ["overlap", "misaligned", "uneven-gap", "tiny-target", "off-grid"]}, "description": "Checks to run (default: all but off-grid)"}), "grid": int("off-grid step in px (default 4)"), "limit": int("Findings to list (default 100, up to 5000)")}),
                &[],
            ),
            true,
        ),
        tool(
            "get_code_connect_map",
            "Read Code Connect links",
            "Which code components the design components are linked to (Code Connect). With `id` (a component, a component set or an instance): the link for that layer, with the import line and the snippet an instance renders to (engine `inspect.componentCode`: {component, name, properties, connected: {importPath, name, template, import, code}, usage?}). Without `id`: every link in the file (`codeConnect.export`: {version, links: [{component, language, importPath, name, template}]}), filtered by `language` (react, leptos, vue, swiftui, compose, arkui, flutter). A component with no link is not connected: link it with add_code_connect_map. Use these snippets in generated code instead of rebuilding a linked component.",
            obj(json!({"id": id_schema("A component, component set or instance"), "language": string("react, leptos, vue, swiftui, compose, arkui or flutter")}), &[]),
            true,
        ),
        tool(
            "add_code_connect_map",
            "Link a component to code",
            "Link a design component (a variant links its set) to the code component it is built as (engine `codeConnect.set`, one undo step). `name` and `importPath` say what to import; `template` is the snippet an instance renders to, with `{property}` standing for the instance's value of that component property (e.g. <Button variant=\"{variant}\">{label}</Button>); `{prop ? \"a\" : \"b\"}` picks one of two strings. Fields left out keep their value; with no template the call passes every property. Returns {component, language, code, unknown} (`unknown` lists placeholders that are not properties).",
            obj(
                json!({"id": id_schema("The component or component set"), "language": string("react (default), leptos, vue, swiftui, compose, arkui or flutter"), "importPath": string("Module to import from, e.g. @acme/ui"), "name": string("The code component's name"), "template": string("Snippet with {property} placeholders")}),
                &["id"],
            ),
            false,
        ),
        tool(
            "diff_documents",
            "Diff two documents",
            "A structural diff (engine `doc.diff`): `a` and `b`, two open files (ids from doc.list; `b` defaults to the file in front), or `version` (a version id from list_versions) against the current file (or `document`). Layers match by id for a version and by page/name path for two files. Returns {mode, a, b, identical, counts: {added, removed, moved, changed}, changed: [{id, path, type, changes: [{prop, old, new}], moreProps?}], moved: [{id, path, from, to}], added: [{id, path, type, layers}], removed: [...], truncated?, note?}: the changes run from a (or the version) to b, at most `limit` items listed, and added/removed list the top layer of each subtree. For one frame's detailed property changes since a version use compare_version. Z-order changes are not reported.",
            obj(
                json!({
                    "a": id_schema("The earlier file: an open document id (doc.list)"),
                    "b": id_schema("The later file (default: the file in front, or `document`)"),
                    "version": string("A version id of the current file, instead of `a` (list_versions)"),
                    "limit": int("Items to list (default 50, up to 1000)")
                }),
                &[],
            ),
            true,
        ),
        tool(
            "summarize_document",
            "Summarize the file",
            "A token-budgeted overview of the file (engine `inspect.summary`) to read before anything else in an unfamiliar file: counts of pages, layers, components, instances, styles and variables; per page the top-level frames with size, child count and layer count; the largest text (headings); repeated element families; and the calls to drill in with. It meets `budgetTokens` (default 2000) by showing less (fewer frames, headings and families, shorter text), never by cutting off mid-list: what is left out is counted in the more* fields, and `level` says how much detail is shown (0 is the most). `overBudget` is set when even the smallest summary does not fit.",
            obj(json!({"budgetTokens": json!({"type": "integer", "minimum": 100, "maximum": 50000, "description": "Token budget for the answer (default 2000)"})}), &[]),
            true,
        ),
        tool(
            "check_accessibility",
            "Audit accessibility",
            "Accessibility audit beyond contrast (engine `a11y.audit`): text contrast (as check_contrast), tap-target size (WCAG 2.5.8: 24 px, 44 px in frames up to 500 wide) for layers with interactions and components named like buttons, links, inputs; non-text contrast (1.4.11: 3:1 for a control's boundary and icons); images and icons without alt text (set with set_properties {alt}; \"\" marks decorative); and the screen-reader reading order count. Returns {issues, checks: {contrast, targets, nonText, alt, readingOrder} (counts), details: {contrast, targets, nonText, alt} (the first `limit` findings each)}. For one check or the full reading order run_command a11y.checkTargets, a11y.checkNonText, a11y.checkAlt or a11y.readingOrder. Limited to `root` (a layer) or `page` (id, name or \"all\"; default the open page).",
            obj(
                json!({"root": id_schema("Only this layer and what is inside it"), "page": string("A page id or name, or \"all\" (default: the open page)"), "limit": int("Findings listed per check (default 100, up to 5000)")}),
                &[],
            ),
            true,
        ),
    ]
}

// ---------- helpers ----------

/// A backend whose every call names `document` (a tool's `document` argument).
struct InDocument<'a> {
    inner: &'a mut dyn Backend,
    document: Value,
}

impl Backend for InDocument<'_> {
    fn call(&mut self, method: &str, mut params: Value) -> Result<Value, String> {
        if let Some(o) = params.as_object_mut() {
            o.insert("document".into(), self.document.clone());
        }
        self.inner.call(method, params)
    }

    fn has_ui(&self) -> bool {
        self.inner.has_ui()
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn detached(&mut self) -> Option<String> {
        self.inner.detached()
    }

    fn target(&self) -> Option<Value> {
        self.inner.target()
    }

    fn set_target(&mut self, doc: Option<Value>) {
        self.inner.set_target(doc);
    }

    fn notice(&mut self) -> Option<String> {
        self.inner.notice()
    }
}

fn exec(b: &mut dyn Backend, cmd: &str, params: Value) -> Result<Value, String> {
    b.call("engine.execute", json!({"command": cmd, "params": params}))
}

/// Run a read command with the tool's arguments; Figma's `nodeId` is the engine's `id`.
fn read_command(b: &mut dyn Backend, cmd: &str, a: &Map<String, Value>) -> Result<Value, String> {
    let mut params = a.clone();
    if let Some(n) = params.remove("nodeId") {
        params.entry("id").or_insert(n);
    }
    exec(b, cmd, Value::Object(params))
}

fn wait_for_changes(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    b.call("events.poll", Value::Object(a.clone())).map(|v| ToolResult::json(&v))
}

fn args_obj(args: &Value) -> Result<Map<String, Value>, String> {
    match args {
        Value::Object(o) => Ok(o.clone()),
        Value::Null => Ok(Map::new()),
        _ => Err("`arguments` must be an object".into()),
    }
}

fn s_arg<'a>(a: &'a Map<String, Value>, k: &str) -> Result<Option<&'a str>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("`{k}` must be a string")),
    }
}

fn req_s<'a>(a: &'a Map<String, Value>, k: &str) -> Result<&'a str, String> {
    s_arg(a, k)?.ok_or_else(|| format!("`{k}` is required"))
}

fn n_arg(a: &Map<String, Value>, k: &str) -> Result<Option<f64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => figcraft_engine::params::f64_of(v).map(Some).ok_or_else(|| format!("`{k}` must be a finite number")),
    }
}

fn id_arg(v: &Value) -> Result<u64, String> {
    figcraft_engine::params::id_of(v).map(|i| i.0).ok_or_else(|| format!("{v} is not a layer id"))
}

fn ids_arg(a: &Map<String, Value>, k: &str) -> Result<Option<Vec<u64>>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(arr)) => arr.iter().map(id_arg).collect::<Result<Vec<_>, _>>().map(Some),
        Some(v) => id_arg(v).map(|i| Some(vec![i])),
    }
}

fn png_info(bytes: &[u8]) -> (u32, u32) {
    crate::png_size(bytes).unwrap_or((0, 0))
}

/// Union of the layers' absolute bounds (page coordinates).
fn bounds_of(b: &mut dyn Backend, ids: &[u64]) -> Option<(f64, f64, f64, f64)> {
    let mut acc: Option<(f64, f64, f64, f64)> = None;
    for id in ids.iter().take(500) {
        let Ok(n) = b.call("node.get", json!({"id": id, "depth": 0})) else { continue };
        let Some(ab) = n.get("absoluteBounds") else { continue };
        let g = |k: &str| ab.get(k).and_then(Value::as_f64);
        let (Some(x), Some(y), Some(w), Some(h)) = (g("x"), g("y"), g("width"), g("height")) else { continue };
        acc = Some(match acc {
            None => (x, y, x + w, y + h),
            Some((a, b2, c, d)) => (a.min(x), b2.min(y), c.max(x + w), d.max(y + h)),
        });
    }
    acc
}

/// Make paint/effect JSON friendlier than the engine's strict serde (see
/// [`figcraft_engine::params::normalize_paint`]). The engine reads paints this way itself; effects
/// still need it here.
pub fn normalize_props(props: &mut Map<String, Value>) {
    for k in ["fills", "strokes", "effects"] {
        if let Some(v) = props.get_mut(k) {
            normalize_paint(v);
        }
    }
}

/// [`normalize_props`] for command params: `props`, flat node.set keys, `paint`, `effect`.
fn friendly_params(p: &mut Value) {
    let Some(o) = p.as_object_mut() else { return };
    if let Some(Value::Object(props)) = o.get_mut("props") {
        normalize_props(props);
    }
    normalize_props(o);
    for k in ["paint", "effect"] {
        if let Some(v) = o.get_mut(k) {
            normalize_paint(v);
        }
    }
}

// ---------- dispatch ----------

/// Refuse argument names `schema` doesn't list, at the top and in the objects it describes inside
/// (a batch's steps); objects it leaves open (`params`) take any. `path` leads the names in the
/// error (`commands[0].`).
fn refuse_unknown_args(schema: &Value, args: &Value, path: &str) -> Result<(), String> {
    if let (Some(props), Some(given)) = (schema.get("properties").and_then(Value::as_object), args.as_object()) {
        // A schema that says so takes further keys (create_node: any set_properties key beside `type`).
        let open = schema.get("additionalProperties") == Some(&Value::Bool(true));
        let unknown: Vec<String> = given.keys().filter(|k| !open && !props.contains_key(*k)).map(|k| format!("`{path}{k}`")).collect();
        if !unknown.is_empty() {
            let s = if unknown.len() == 1 { "" } else { "s" };
            let takes: Vec<&str> = props.keys().map(String::as_str).collect();
            let takes = if takes.is_empty() { "none".to_string() } else { takes.join(", ") };
            return Err(format!("unknown argument{s} {} (it takes {takes})", unknown.join(", ")));
        }
        for (k, v) in given {
            if let Some(inner) = props.get(k) {
                refuse_unknown_args(inner, v, &format!("{path}{k}."))?;
            }
        }
    }
    if let (Some(items), Some(list)) = (schema.get("items"), args.as_array()) {
        for (i, v) in list.iter().enumerate() {
            refuse_unknown_args(items, v, &format!("{}[{i}].", path.trim_end_matches('.')))?;
        }
    }
    Ok(())
}

/// Run one tool. Every failure becomes an `isError` result (never a panic).
pub fn call_tool(b: &mut dyn Backend, name: &str, args: &Value) -> ToolResult {
    let mut a = match args_obj(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(e),
    };
    // Built once: the catalogue is the same on every call.
    static TOOLS: std::sync::LazyLock<Vec<Value>> = std::sync::LazyLock::new(tool_definitions);
    if let Some(schema) = TOOLS.iter().find(|t| t.get("name").and_then(Value::as_str) == Some(name)).and_then(|t| t.get("inputSchema"))
        && let Err(e) = refuse_unknown_args(schema, &Value::Object(a.clone()), "")
    {
        return ToolResult::error(e);
    }
    let mut r = dispatch(b, name, &mut a).unwrap_or_else(ToolResult::error);
    if let Some(too_big) = oversize(name, &r) {
        r = ToolResult::error(too_big);
    }
    // A detached copy beside an open app: the result carries the warning, not only the tool list.
    if name != "list_commands"
        && let Some(note) = b.detached()
    {
        r.content.push(json!({"type": "text", "text": note}));
    }
    if let Some(note) = b.notice() {
        r.content.push(json!({"type": "text", "text": note}));
    }
    r
}

/// Most text one tool result may carry, and most image bytes (base64).
const MAX_RESULT_TEXT: usize = 8 << 20;
const MAX_RESULT_IMAGE: usize = 16 << 20;

/// The error that replaces a result no client could read: it names the size and the ways to ask for less.
pub(crate) fn oversize(name: &str, r: &ToolResult) -> Option<String> {
    let len = |key: &str, kind: &str| -> usize {
        r.content
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some(kind))
            .filter_map(|c| c.get(key).and_then(Value::as_str))
            .map(str::len)
            .sum()
    };
    let (text, image) = (len("text", "text"), len("data", "image"));
    if text <= MAX_RESULT_TEXT && image <= MAX_RESULT_IMAGE {
        return None;
    }
    let mib = |n: usize| n.div_ceil(1 << 20);
    Some(format!(
        "{name} produced {} MiB of {}, more than one result can carry ({} MiB). Ask for less: get_node with a smaller depth or `fields`, find_nodes with a lower `limit`, export or screenshot with a lower `scale`/`maxSize` or a `path` to write a file.",
        mib(text.max(image)),
        if text > MAX_RESULT_TEXT { "text" } else { "image data" },
        mib(if text > MAX_RESULT_TEXT { MAX_RESULT_TEXT } else { MAX_RESULT_IMAGE }),
    ))
}

/// Whether `d` can name an open file: an id, or a path or name.
fn valid_document(d: &Value) -> bool {
    d.is_u64() || d.as_str().is_some_and(|s| !s.is_empty())
}

/// Pick the file(s) a tool call acts on: `documents` (one call per file), else `document`, else
/// the session's target document, else the file in front.
fn dispatch(b: &mut dyn Backend, name: &str, a: &mut Map<String, Value>) -> Result<ToolResult, String> {
    let documents = a.remove("documents").filter(|d| !d.is_null());
    // `use_document` reads its own `document`.
    let explicit = if name == "use_document" { None } else { a.remove("document").filter(|d| !d.is_null()) };
    if let Some(documents) = documents {
        if explicit.is_some() {
            return Err("pass `document` or `documents`, not both".into());
        }
        return across_documents(b, name, a, &documents);
    }
    // A capture of the app window shows the file in front, whatever the target is.
    let window_shot = name == "screenshot" && a.get("window").is_some_and(|w| !w.is_null() && *w != Value::Bool(false));
    let by_target = explicit.is_none() && !window_shot && !UNTARGETED.contains(&name);
    let Some(document) = explicit.or_else(|| if by_target { b.target() } else { None }) else {
        return run_tool(b, name, a, None);
    };
    let r = run_tool(&mut InDocument { inner: &mut *b, document: document.clone() }, name, a, Some(&document));
    // A target whose tab was closed would fail every call: say so and go back to the tab in front.
    if r.is_err() && by_target && status_row(b, Some(&document)).is_none() {
        b.set_target(None);
        return Err(format!(
            "the target document {document} is no longer open; the target is cleared, so calls act on the file in front again (use_document sets another)"
        ));
    }
    r
}

/// `documents`: the tool once in each file, collected as `{documents: [{document, name, ok, result
/// | error}], totalMatched?}`; a file where it fails does not stop the others.
fn across_documents(b: &mut dyn Backend, tool: &str, a: &Map<String, Value>, documents: &Value) -> Result<ToolResult, String> {
    let rows = exec(b, "doc.list", json!({}))?;
    let rows = rows.as_array().map(Vec::as_slice).unwrap_or_default();
    let name_of = |id: &Value| rows.iter().find(|r| r.get("id") == Some(id)).and_then(|r| r.get("name")).cloned().unwrap_or(Value::Null);
    let wanted: Vec<Value> = match documents {
        Value::String(s) if s == "all" => rows.iter().filter_map(|r| r.get("id").cloned()).collect(),
        Value::Array(list) if list.iter().all(valid_document) => list.clone(),
        _ => return Err("`documents` must be \"all\" or a list of document ids (doc.list)".into()),
    };
    if wanted.is_empty() {
        return Err("`documents` names no open document".into());
    }
    if wanted.len() > MAX_DOCUMENTS {
        return Err(format!("{} documents: at most {MAX_DOCUMENTS} per call", wanted.len()));
    }
    // find_nodes counts `total`, the layer tools `matched`.
    let count_key = if tool == "find_nodes" { "total" } else { "matched" };
    let (mut results, mut succeeded, mut sum, mut counted) = (Vec::new(), 0usize, 0u64, false);
    for document in &wanted {
        let outcome = run_tool(&mut InDocument { inner: &mut *b, document: document.clone() }, tool, a, Some(document));
        let mut entry = json!({"document": document, "name": name_of(document)});
        let outcome = match outcome {
            Ok(r) if !r.is_error => Ok(serde_json::from_str::<Value>(r.first_text()).unwrap_or_else(|_| json!(r.first_text()))),
            Ok(r) => Err(r.first_text().to_string()),
            // Nothing to change in a file that lacks the element is not a failure of the call.
            Err(e) if tool == "set_properties" && e.starts_with("no layer matches") => Ok(json!({"matched": 0, "ids": [], "note": e})),
            Err(e) => Err(e),
        };
        match outcome {
            Ok(v) => {
                succeeded += 1;
                if let Some(n) = v.get(count_key).and_then(Value::as_u64) {
                    sum += n;
                    counted = true;
                }
                entry["ok"] = json!(true);
                entry["result"] = v;
            }
            Err(e) => {
                entry["ok"] = json!(false);
                entry["error"] = json!(e);
            }
        }
        results.push(entry);
    }
    let mut out = json!({"documents": results});
    if counted {
        out["totalMatched"] = json!(sum);
        if tool == "find_nodes" {
            out["total"] = json!(sum);
        }
    }
    let mut r = ToolResult::json(&out);
    r.is_error = succeeded == 0;
    Ok(r)
}

/// One tool in the file `b` addresses. `document` is that file as the caller named it, if any.
fn run_tool(b: &mut dyn Backend, name: &str, a: &Map<String, Value>, doc_target: Option<&Value>) -> Result<ToolResult, String> {
    match name {
        "list_commands" => list_commands(b, a),
        "run_command" => run_command(b, a),
        "batch" => batch(b, a),
        "create_node" => create_node(b, a),
        "set_properties" => set_properties(b, a),
        "inspect_document" => inspect_document(b, a),
        "get_node" => get_node(b, a),
        "select" => select(b, a),
        "get_selection" => get_selection(b),
        "find_nodes" => find_nodes(b, a),
        "get_nodes" => get_nodes(b, a),
        "contact_sheet" => contact_sheet(b, a),
        "export" => export(b, a),
        "export_source" => export_source(b, a),
        "export_many" => export_many(b, a),
        "screenshot" => screenshot(b, a),
        "open_file" => open_file(b, a),
        "save_file" => save_file(b, a, doc_target),
        "undo" => exec(b, "edit.undo", json!({})).map(|v| ToolResult::json(&v)),
        "redo" => exec(b, "edit.redo", json!({})).map(|v| ToolResult::json(&v)),
        "pointer" => pointer(b, a),
        "key" => key(b, a),
        "type_text" => type_text(b, a),
        "inspect_ui" => b.call("ui.inspect", json!({})).map(|mut v| {
            // Which server answers: with several running, the way to tell them apart.
            if let Some(o) = v.as_object_mut() {
                o.insert("mcpServer".into(), json!({"pid": std::process::id(), "backend": b.describe()}));
            }
            ToolResult::json(&v)
        }),
        "get_metadata" => read_command(b, "inspect.metadata", a).map(|v| match v.get("xml").and_then(Value::as_str) {
            Some(xml) if v["truncated"] == true => {
                ToolResult::text(format!("{xml}<!-- truncated: raise depth or maxNodes, or ask for a child's nodeId -->\n"))
            }
            Some(xml) => ToolResult::text(xml),
            None => ToolResult::json(&v),
        }),
        "get_variable_defs" => read_command(b, "inspect.variableDefs", a).map(|v| ToolResult::json(&v["variables"])),
        "get_design_context" => read_command(b, "inspect.designContext", a).map(|v| ToolResult::json(&v)),
        "search_design_system" => read_command(b, "inspect.searchDesignSystem", a).map(|v| ToolResult::json(&v)),
        "lint_icons" => lint_icons(b, a),
        "export_tokens" => export_tokens(b, a),
        "check_contrast" => check_contrast(b, a),
        "history" => history(b, a),
        "auto_name_layers" => auto_name_layers(b, a),
        "wait_for_changes" => wait_for_changes(b, a),
        "diagnostics" => diagnostics(b),
        "use_document" => use_document(b, a),
        "copy_between_documents" => copy_between_documents(b, a),
        "checkpoint" => checkpoint(b, a),
        "list_versions" => list_versions(b, a),
        "restore_version" => restore_version(b, a),
        "compare_version" => compare_version(b, a),
        "find_similar" => find_similar(b, a),
        "propagate_change" => propagate_change(b, a),
        "get_code_connect_map" => get_code_connect_map(b, a),
        "add_code_connect_map" => {
            exec(b, "codeConnect.set", pick(a, &["id", "language", "importPath", "name", "template"])).map(|v| ToolResult::json(&v))
        }
        "audit_tokens" => exec(b, "lint.tokens", pick(a, &["root", "page", "limit"])).map(|v| ToolResult::json(&v)),
        "lint_layout" => exec(b, "lint.layout", pick(a, &["root", "page", "checks", "grid", "limit"])).map(|v| ToolResult::json(&v)),
        "diff_documents" => exec(b, "doc.diff", pick(a, &["a", "b", "version", "limit"])).map(|v| ToolResult::json(&v)),
        "check_accessibility" => exec(b, "a11y.audit", pick(a, &["root", "page", "limit"])).map(|v| ToolResult::json(&v)),
        "summarize_document" => exec(b, "inspect.summary", pick(a, &["budgetTokens"])).map(|v| ToolResult::json(&v)),
        other => Err(format!("unknown tool `{other}` (tools/list has: {})", TOOL_NAMES.join(", "))),
    }
}

fn list_commands(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let filter = s_arg(a, "filter")?.map(str::to_lowercase).unwrap_or_default();
    let all = b.call("engine.commands", json!({}))?;
    let list: Vec<Value> = all
        .as_array()
        .map(|v| {
            v.iter()
                .filter(|c| {
                    filter.is_empty()
                        || ["id", "label", "menu", "params"]
                            .iter()
                            .any(|k| c.get(*k).and_then(Value::as_str).is_some_and(|s| s.to_lowercase().contains(&filter)))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if list.is_empty() {
        return Ok(ToolResult::text(format!("no command matches `{filter}`")));
    }
    let count = |key: &str, default: usize| -> Result<usize, String> {
        match a.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| format!("`{key}` must be a whole number")),
        }
    };
    let limit = count("limit", 400)?.clamp(1, 2000);
    let offset = count("offset", 0)?;
    let total = list.len();
    let end = (offset + limit).min(total);
    if offset >= total {
        return Err(format!("`offset` {offset} is past the {total} matching commands"));
    }
    // One compact line per command keeps this readable for agents.
    let mut out = format!("{total} commands (id — label [menu] (shortcut) params; * = undoable):\n");
    for c in &list[offset..end] {
        let g = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
        out.push_str(g("id"));
        if c.get("undoable").and_then(Value::as_bool) == Some(true) {
            out.push('*');
        }
        out.push_str(" — ");
        out.push_str(g("label"));
        if !g("menu").is_empty() {
            out.push_str(&format!(" [{}]", g("menu")));
        }
        if !g("shortcut").is_empty() {
            out.push_str(&format!(" ({})", g("shortcut")));
        }
        out.push(' ');
        out.push_str(g("params"));
        out.push('\n');
    }
    if end < total || offset > 0 {
        out.push_str(&format!("showing {}-{end} of {total}; pass offset (and limit) or a filter for the rest\n", offset + 1));
    }
    Ok(ToolResult::text(out))
}

fn run_command(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let cmd = req_s(a, "command")?;
    let params = match a.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err("`params` must be an object".into()),
    };
    let mut params = params;
    friendly_params(&mut params);
    exec(b, cmd, params).map(|v| ToolResult::json(&v))
}

fn batch(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let Some(Value::Array(cmds)) = a.get("commands") else { return Err("`commands` must be an array of {command, params}".into()) };
    if cmds.is_empty() {
        return Err("`commands` is empty".into());
    }
    let mut steps = Vec::with_capacity(cmds.len());
    for (i, c) in cmds.iter().enumerate() {
        let Some(cmd) = c.get("command").and_then(Value::as_str) else { return Err(format!("step {i}: `command` must be a string")) };
        let params = match c.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err(format!("step {i} ({cmd}): `params` must be an object")),
        };
        let mut params = params;
        friendly_params(&mut params);
        let mut step = json!({"command": cmd, "params": params});
        if let (Some(name), Some(o)) = (c.get("as"), step.as_object_mut()) {
            o.insert("as".into(), name.clone());
        }
        steps.push(step);
    }
    let label = s_arg(a, "label")?.unwrap_or("Batch");
    let mut params = json!({"label": label, "commands": steps});
    if let Some(mode) = a.get("onError") {
        params["onError"] = mode.clone();
    }
    let quiet = a.get("quiet").and_then(Value::as_bool).unwrap_or(false);
    exec(b, "command.batch", params).map(|v| ToolResult::json(&if quiet { quiet_batch(&v) } else { v }))
}

/// `{ok, count, failures?}` for a batch result: only the `ok: false` outcomes of `onError: continue` are kept.
fn quiet_batch(results: &Value) -> Value {
    let all = results.as_array().map(Vec::as_slice).unwrap_or_default();
    let failures: Vec<&Value> = all.iter().filter(|r| r.get("ok") == Some(&Value::Bool(false))).collect();
    let mut out = json!({"ok": failures.is_empty(), "count": all.len()});
    if !failures.is_empty() {
        out["failures"] = json!(failures);
    }
    out
}

fn create_node(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    req_s(a, "type")?;
    for k in ["x", "y", "width", "height"] {
        n_arg(a, k)?;
    }
    if let Some(p) = a.get("props")
        && !p.is_object()
        && !p.is_null()
    {
        return Err("`props` must be an object".into());
    }
    let dry = flag(a, "dryRun")?;
    flag(a, "keepSelection")?;
    let mut params = Map::new();
    const KNOWN: [&str; 18] = [
        "type",
        "x",
        "y",
        "width",
        "height",
        "parent",
        "index",
        "name",
        "key",
        "text",
        "path",
        "points",
        "closed",
        "props",
        "select",
        "absolute",
        "dryRun",
        "keepSelection",
    ];
    for k in KNOWN {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            params.insert(k.into(), v.clone());
        }
    }
    // Style keys given beside `type` (fill, stroke, layout, ...) are `props`; `props` wins on a clash.
    let mut props = Map::new();
    for (k, v) in a.iter().filter(|(k, v)| !KNOWN.contains(&k.as_str()) && !v.is_null()) {
        props.insert(k.clone(), v.clone());
    }
    if let Some(Value::Object(p)) = params.remove("props") {
        props.extend(p);
    }
    if !props.is_empty() {
        normalize_props(&mut props);
        params.insert("props".into(), Value::Object(props));
    }
    let r = exec(b, "node.create", Value::Object(params))?;
    if dry {
        return Ok(ToolResult::json(&r));
    }
    let id = r.get("id").cloned().ok_or("node.create returned no id")?;
    let node = b.call("node.get", json!({"id": id, "depth": 0}))?;
    Ok(ToolResult::json(&json!({"id": id, "node": node})))
}

fn set_properties(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let props = match a.get("props") {
        Some(Value::Object(p)) if !p.is_empty() => p.clone(),
        _ => return Err("`props` must be a non-empty object".into()),
    };
    let mut props = props;
    normalize_props(&mut props);
    let mut params = json!({"props": props});
    let (dry, keep) = (flag(a, "dryRun")?, flag(a, "keepSelection")?);
    for (k, on) in [("dryRun", dry), ("keepSelection", keep)] {
        if on {
            params[k] = json!(true);
        }
    }
    let found = named_layers(b, a)?;
    let matched = found.as_ref().map(|f| f.1);
    let ids = match found {
        Some((ids, _)) => Some(ids),
        None => ids_arg(a, "ids")?,
    };
    if let Some(ids) = &ids {
        if ids.is_empty() {
            let why = if matched.is_some() { "no layer matches `where` or `list`: nothing changed" } else { "`ids` is empty" };
            return Err(why.into());
        }
        params["ids"] = json!(ids);
    }
    let result = exec(b, "node.set", params)?;
    if dry {
        let mut out = if result.is_object() { result } else { json!({"dryRun": true, "result": result}) };
        if let Some(m) = matched {
            out["matched"] = json!(m);
            out["ids"] = json!(ids);
        }
        return Ok(ToolResult::json(&out));
    }
    let warnings = result.get("warnings").cloned();
    let ids = match ids {
        Some(i) => i,
        None => b
            .call("document.inspect", json!({"depth": 0}))?
            .get("selection")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default(),
    };
    let quiet = match a.get("quiet") {
        None | Some(Value::Null) => ids.len() > 1,
        Some(Value::Bool(q)) => *q,
        Some(_) => return Err("`quiet` must be a boolean".into()),
    };
    let mut out = if quiet {
        json!({"ok": true, "ids": ids})
    } else {
        let nodes: Vec<Value> = ids.iter().take(20).filter_map(|i| b.call("node.get", json!({"id": i, "depth": 0})).ok()).collect();
        json!({"ok": true, "nodes": nodes})
    };
    if let Some(m) = matched {
        out["matched"] = json!(m);
        out["ids"] = json!(ids);
    }
    if let Some(w) = warnings {
        out["warnings"] = w;
    }
    if !quiet && let Some(similar) = similar_on(b, &ids) {
        out["similarOn"] = similar;
    }
    Ok(ToolResult::json(&out))
}

/// Where else the edited layer has twins: up to 5 close matches on other frames, with the call that
/// would carry the change over. Only for an edit of one to three layers; a failure says nothing.
fn similar_on(b: &mut dyn Backend, ids: &[u64]) -> Option<Value> {
    if ids.is_empty() || ids.len() > 3 {
        return None;
    }
    let found = exec(b, "family.find", json!({"id": ids[0], "minScore": 0.85, "limit": 5})).ok()?;
    let twins: Vec<Value> = found.get("twins")?.as_array()?.iter().filter(|t| t["sameFrame"] != true).cloned().collect();
    if twins.is_empty() {
        return None;
    }
    Some(
        json!({"from": ids[0], "total": found["total"], "twins": twins, "hint": "these layers look like the same element on other frames; propagate_change {from, confirm: true} copies this change to them (leave confirm out to preview)"}),
    )
}

/// The layers `where` (the `node.find` filters) names, with how many matched; `None` without it.
/// An error when `ids` is given too, when `where` filters nothing, or when more layers match than
/// `where.limit` returns, so nothing is applied to part of a set.
fn where_arg(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<Option<(Vec<u64>, u64)>, String> {
    let Some((ids, total)) = where_find(b, a, None)? else { return Ok(None) };
    if total > ids.len() as u64 {
        return Err(format!(
            "{total} layers match `where` but `where.limit` allows {}: narrow the filters or raise the limit (up to 10000)",
            ids.len()
        ));
    }
    Ok(Some((ids, total)))
}

/// A boolean argument, false when absent.
fn flag(a: &Map<String, Value>, k: &str) -> Result<bool, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(x)) => Ok(*x),
        Some(_) => Err(format!("`{k}` must be a boolean")),
    }
}

/// The ids of the saved list `name` (`list.get`).
fn list_ids(b: &mut dyn Backend, name: &str) -> Result<Vec<u64>, String> {
    let r = exec(b, "list.get", json!({"name": name}))?;
    let items = match &r {
        Value::Array(items) => Some(items),
        other => other.get("ids").or_else(|| other.get("nodes")).and_then(Value::as_array),
    };
    let items = items.ok_or_else(|| format!("list `{name}` has no layers in the engine's answer: {r}"))?;
    Ok(items.iter().filter_map(|v| v.as_u64().or_else(|| v.get("id").and_then(Value::as_u64))).collect())
}

/// The layers `where` or `list` names, with how many there are; `None` with neither. `where` is as
/// for [`where_arg`] (more matches than its limit is an error).
fn named_layers(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<Option<(Vec<u64>, u64)>, String> {
    let Some(name) = s_arg(a, "list")? else { return where_arg(b, a) };
    if a.get("where").is_some_and(|v| !v.is_null()) {
        return Err("pass `where` or `list`, not both".into());
    }
    if a.get("ids").is_some_and(|v| !v.is_null()) {
        return Err("pass `ids` or `list`, not both".into());
    }
    let ids = list_ids(b, name)?;
    let n = ids.len() as u64;
    Ok(Some((ids, n)))
}

/// Layers named by `ids`, `where` or `list` (exactly one), cut at `cap`: the ids and how many
/// there are in all. `where` takes `cap` as its default limit and is not an error when more match.
fn targets(b: &mut dyn Backend, a: &Map<String, Value>, cap: usize) -> Result<(Vec<u64>, u64), String> {
    let given = ["ids", "where", "list"].iter().filter(|k| a.get(**k).is_some_and(|v| !v.is_null())).count();
    if given != 1 {
        return Err("pass exactly one of `ids`, `where` and `list`".into());
    }
    let (mut ids, total) = if let Some(name) = s_arg(a, "list")? {
        let ids = list_ids(b, name)?;
        let n = ids.len() as u64;
        (ids, n)
    } else if let Some(found) = where_find(b, a, Some(cap))? {
        found
    } else {
        let ids = ids_arg(a, "ids")?.unwrap_or_default();
        let n = ids.len() as u64;
        (ids, n)
    };
    ids.truncate(cap);
    Ok((ids, total))
}

/// [`where_arg`] without the limit check: the ids found (at most `where.limit`, by default
/// `default_limit`) and the number of matches.
fn where_find(b: &mut dyn Backend, a: &Map<String, Value>, default_limit: Option<usize>) -> Result<Option<(Vec<u64>, u64)>, String> {
    let Some(w) = a.get("where").filter(|v| !v.is_null()) else { return Ok(None) };
    if a.get("ids").is_some_and(|v| !v.is_null()) {
        return Err("pass `ids` or `where`, not both".into());
    }
    let Value::Object(filters) = w else { return Err("`where` must be an object of node.find filters".into()) };
    let mut filters = filters.clone();
    filters.retain(|_, v| !v.is_null());
    if filters.keys().all(|k| k == "limit") {
        return Err("`where` needs a filter besides `limit` (it would match every layer)".into());
    }
    if let Some(l) = default_limit {
        filters.entry("limit").or_insert(json!(l));
    }
    let r = exec(b, "node.find", Value::Object(filters))?;
    let ids: Vec<u64> = r.get("ids").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    let total = r.get("total").and_then(Value::as_u64).unwrap_or(ids.len() as u64);
    Ok(Some((ids, total)))
}

/// What one tool result may take on the wire. MCP stdio is newline-delimited JSON, so a result is
/// a single message and has to fit the client's read buffer whole -- there is no streaming of part
/// of one. The TypeScript SDK's stdio transport defaults to 10 MiB
/// (`STDIO_DEFAULT_MAX_BUFFER_SIZE`) and hosts rarely raise it; a document whose tree went past
/// that killed the server, because the client stopped reading, the pipe broke, and every later
/// call failed (#912). The 2 MiB left over covers the JSON-RPC envelope.
const MAX_RESPONSE: usize = 8 << 20;

/// Room kept for the `truncated` object, which is added after the size is measured and would
/// otherwise be able to push a response that only just fitted back over the limit.
const MARKER_RESERVE: usize = 1 << 10;

/// Room kept for everything the text block is wrapped in: the content object and `isError`, the
/// JSON-RPC envelope (`jsonrpc`, `id`, `result`) and the newline the transport writes after it.
/// About ninety bytes of that is fixed and the rest is the request id, so `maxBytes` holds for
/// any id up to roughly 900 bytes -- which is every id in practice, a number or a UUID. An id
/// larger than that is the one case where the message can pass the caller's figure, and sizing
/// the real envelope here would mean handing the transport's request id to every tool.
const ENVELOPE_RESERVE: usize = 1 << 10;

/// What the caller says it can read, as the bytes its *tree* may take: the figure is for the whole
/// message, so the envelope's share comes off it here.
fn budget(a: &Map<String, Value>) -> Result<(usize, usize), String> {
    // A caller that knows its own buffer can say so; 1 KiB is below the shallowest tree, so the
    // floor only keeps the arithmetic sane.
    let max_bytes = match n_arg(a, "maxBytes")? {
        Some(v) if v < 1024.0 => return Err("`maxBytes` must be at least 1024".into()),
        Some(v) => v.min(usize::MAX as f64) as usize,
        None => MAX_RESPONSE,
    };
    Ok((max_bytes, max_bytes.saturating_sub(ENVELOPE_RESERVE)))
}

/// What a value's text block would take on the wire. It is sent as a text block, so it is measured
/// pretty-printed *and* escaped -- escaping adds about a tenth.
fn wire_size(value: &Value) -> usize {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    serde_json::to_string(&text).map_or(text.len(), |s| s.len())
}

/// The document at `depth`, and what its text block would take on the wire. The tree is sent as a
/// text block, so it is measured pretty-printed *and* escaped -- escaping adds about a tenth.
fn inspect_at(b: &mut dyn Backend, depth: u64, all: bool, options: &Value) -> Result<(Value, usize), String> {
    let mut params = options.clone();
    params["depth"] = json!(depth);
    params["allPages"] = json!(all);
    let value = b.call("document.inspect", params)?;
    let wire = wire_size(&value);
    Ok((value, wire))
}

/// One layer's subtree at `depth`, and what its text block would take on the wire.
fn node_at(b: &mut dyn Backend, id: u64, depth: u64, options: &Value) -> Result<(Value, usize), String> {
    let mut params = options.clone();
    params["id"] = json!(id);
    params["depth"] = json!(depth);
    let value = b.call("node.get", params)?;
    let wire = wire_size(&value);
    Ok((value, wire))
}

fn inspection_options(a: &Map<String, Value>) -> Result<Value, String> {
    let options = Value::Object(a.iter().filter(|(k, _)| matches!(k.as_str(), "fields" | "outline")).map(|(k, v)| (k.clone(), v.clone())).collect());
    figcraft_engine::inspect::InspectOptions::parse(&options)?;
    Ok(options)
}

fn inspect_document(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let depth = n_arg(a, "depth")?.unwrap_or(4.0).clamp(0.0, 64.0) as u64;
    let all = match a.get("allPages") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(x)) => *x,
        Some(_) => return Err("`allPages` must be a boolean".into()),
    };
    let options = inspection_options(a)?;
    let (max_bytes, budget) = budget(a)?;

    let (value, wire) = inspect_at(b, depth, all, &options)?;
    if wire <= budget {
        return Ok(ToolResult::json(&value));
    }

    let (used, mut value, used_wire) = if depth == 0 {
        // Depth 0 is the shallowest tree there is, so there is nothing smaller to send: it goes
        // out over the limit with the marker saying so, the same as a bisection that reaches
        // depth 0. An error would leave the caller with nothing at all.
        (0, value, wire)
    } else {
        // The deepest tree that fits, by bisection: a deeper tree holds every node of a shallower
        // one, so the size does not fall as the depth grows. Bisecting keeps this to about six
        // more walks of the document rather than one per level.
        let (mut lo, mut hi) = (0u64, depth - 1);
        let mut best: Option<(u64, Value, usize)> = None;
        while lo <= hi {
            let mid = lo + (hi - lo) / 2;
            let (v, w) = inspect_at(b, mid, all, &options)?;
            if w + MARKER_RESERVE <= budget {
                best = Some((mid, v, w));
                if mid == hi {
                    break;
                }
                lo = mid + 1;
            } else if mid == 0 {
                best = Some((0, v, w));
                break;
            } else {
                hi = mid - 1;
            }
        }
        match best {
            Some(found) => found,
            None => return Ok(ToolResult::json(&value)),
        }
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "truncated".into(),
            json!({
                "requestedDepth": depth,
                "depth": used,
                "requestedDepthBytes": wire,
                "bytes": used_wire,
                "maxBytes": max_bytes,
                "reason": format!(
                    "the tree at depth {depth} would be {wire} bytes on the wire, over the {max_bytes}-byte limit a client can read in one message; it is reported at depth {used} instead"
                ),
                "hint": "get_node({id, depth}) walks one subtree at a time, within the same limit: a node with more children than fit answers with a window of them and the offset to ask for next",
            }),
        );
    }
    Ok(ToolResult::json(&value))
}

/// One layer's subtree, kept inside what the caller can read.
///
/// `inspect_document` sends a shallower tree when the document's is too large and points here for
/// the rest, so this tool is the way out of that limit and must not be a way past it: a frame with
/// fifteen thousand children answers with 10.5 MiB at depth 1, over the buffer the report in #912
/// is about, and no depth makes that smaller because the width is at depth 1. So the subtree is
/// cut by depth first, as the document's is, and then by a window over the children.
fn get_node(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let id = match (a.get("id"), s_arg(a, "key")?) {
        (Some(v), _) => id_arg(v)?,
        (None, Some(k)) => id_arg(b.call("node.resolveKey", json!({"key": k}))?.get("id").ok_or("no layer has that key")?)?,
        _ => return Err("`id` or `key` is required".into()),
    };
    let depth = n_arg(a, "depth")?.unwrap_or(1.0).clamp(0.0, 64.0) as u64;
    let offset = n_arg(a, "offset")?.unwrap_or(0.0).max(0.0).min(u32::MAX as f64) as usize;
    let options = inspection_options(a)?;
    let (max_bytes, budget) = budget(a)?;

    let (value, wire) = node_at(b, id, depth, &options)?;
    if value.is_null() {
        return Err(format!("no such layer {id}"));
    }
    if wire <= budget && offset == 0 {
        return Ok(ToolResult::json(&value));
    }

    // The deepest subtree that fits, by bisection, as the document's tree is cut. A node asked for
    // at a depth of its own children only (1) cannot be made shallower without losing them all,
    // so that is where the window takes over.
    let (mut used, mut value, mut used_wire) = (depth, value, wire);
    if depth > 1 && offset == 0 {
        let (mut lo, mut hi) = (1u64, depth - 1);
        while lo <= hi {
            let mid = lo + (hi - lo) / 2;
            let (v, w) = node_at(b, id, mid, &options)?;
            if w + MARKER_RESERVE <= budget {
                (used, value, used_wire) = (mid, v, w);
                if mid == hi {
                    break;
                }
                lo = mid + 1;
            } else if mid == 1 {
                (used, value, used_wire) = (1, v, w);
                break;
            } else {
                hi = mid - 1;
            }
        }
        if used_wire + MARKER_RESERVE <= budget {
            return Ok(ToolResult::json(&with_marker(value, Cut { asked: depth, used, asked_wire: wire, used_wire }, max_bytes, true, None)));
        }
    }

    // Still over, or a window was asked for: the children from `offset` that fit.
    let total = value.get("children").and_then(Value::as_array).map_or(0, Vec::len);
    let window = children_window(&mut value, offset, budget.saturating_sub(MARKER_RESERVE));
    let shown = value.get("children").and_then(Value::as_array).map_or(0, Vec::len);
    // A child too large to fit on its own is passed over (a page of just the marker), so paging
    // moves on and every answer stays within maxBytes; `get_node` on the child reads it.
    let after = offset + shown.max(1);
    let next = (after < total).then_some(after);
    let used_wire = wire_size(&value);
    if used_wire + MARKER_RESERVE > budget {
        // Even with every child left out, the layer's own fields (a very long name, say) pass the
        // limit: an answer would be larger than the caller can read, so there is none.
        return Err(format!(
            "layer {id} takes {used_wire} bytes on the wire without its children, more than fit in maxBytes {max_bytes}; ask again with a larger `maxBytes`"
        ));
    }
    // Children left out after the window were cut by size; a window that only `offset` moved, of a
    // subtree within the limit, is paging.
    let over = wire > budget || next.is_some();
    let mut out = with_marker(
        value,
        Cut { asked: depth, used, asked_wire: wire, used_wire },
        max_bytes,
        over,
        Some(json!({
            "from": offset,
            "shown": shown,
            "total": total,
            "nextOffset": next,
        })),
    );
    if window && shown == total && offset == 0 && used == depth {
        // Nothing was left out after all (an offset of zero that fitted whole).
        out.as_object_mut().map(|o| o.remove("truncated"));
    }
    Ok(ToolResult::json(&out))
}

/// Keep the children from `offset` that fit in `budget`, and say whether any were left out. The
/// window is found by bisection: a node with thousands of children would otherwise be serialised
/// once per child.
fn children_window(value: &mut Value, offset: usize, budget: usize) -> bool {
    let Some(all) = value.get("children").and_then(Value::as_array).cloned() else { return false };
    let rest: Vec<Value> = all.iter().skip(offset).cloned().collect();
    let fits = |value: &mut Value, n: usize| {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("children".into(), Value::Array(rest.iter().take(n).cloned().collect()));
        }
        wire_size(value) <= budget
    };
    if fits(value, rest.len()) {
        return rest.len() != all.len();
    }
    // The largest window that fits, between none and all of the rest.
    let (mut lo, mut hi) = (0usize, rest.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(value, mid) { lo = mid } else { hi = mid - 1 }
    }
    fits(value, lo);
    true
}

/// How a subtree was cut: the depth asked for and the depth reported, with their sizes on the wire.
struct Cut {
    asked: u64,
    used: u64,
    asked_wire: usize,
    used_wire: usize,
}

/// `value` with the `truncated` object saying how it was cut. `over`: the subtree asked for passed
/// the limit (else the window is only the `offset` asked for). `children`: the window, when the
/// children were cut as well as (or instead of) the depth.
fn with_marker(mut value: Value, cut: Cut, max_bytes: usize, over: bool, children: Option<Value>) -> Value {
    let Cut { asked, used, asked_wire, used_wire } = cut;
    if let Some(obj) = value.as_object_mut() {
        let mut marker = json!({
            "requestedDepth": asked,
            "depth": used,
            "requestedDepthBytes": asked_wire,
            "bytes": used_wire,
            "maxBytes": max_bytes,
        });
        match &children {
            Some(window) => {
                marker["children"] = window.clone();
                let from = window.get("from").and_then(Value::as_u64).unwrap_or(0);
                marker["reason"] = json!(if over {
                    format!(
                        "the subtree at depth {asked} would be {asked_wire} bytes on the wire, over the {max_bytes}-byte limit a client can read in one message; its children are reported from {from} instead"
                    )
                } else {
                    format!(
                        "its children are reported from {from}, the `offset` asked for; the subtree at depth {asked} ({asked_wire} bytes) is within the {max_bytes}-byte limit"
                    )
                });
                marker["hint"] = json!("ask again with the `offset` in `children.nextOffset` for the rest, or get_node({id, depth}) on one child");
            }
            None => {
                marker["reason"] = json!(format!(
                    "the subtree at depth {asked} would be {asked_wire} bytes on the wire, over the {max_bytes}-byte limit a client can read in one message; it is reported at depth {used} instead"
                ));
                marker["hint"] = json!("get_node({id, depth}) on a child reads deeper, one subtree at a time");
            }
        }
        obj.insert("truncated".into(), marker);
    }
    value
}

fn select(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let found = named_layers(b, a)?;
    let ids = match &found {
        Some((ids, _)) => ids.clone(),
        None => ids_arg(a, "ids")?.ok_or("`ids`, `where` or `list` is required (an empty `ids` array deselects)")?,
    };
    let dry = flag(a, "dryRun")?;
    flag(a, "keepSelection")?;
    // A `where` that finds nothing leaves the user's selection alone instead of clearing it.
    let nothing_found = found.is_some() && ids.is_empty();
    if !nothing_found && !dry {
        if ids.is_empty() {
            exec(b, "select.none", json!({}))?;
        } else {
            exec(b, "select.set", json!({"ids": ids}))?;
        }
    }
    let sel = b.call("document.inspect", json!({"depth": 0}))?.get("selection").cloned().unwrap_or(Value::Null);
    let mut out = json!({"selection": sel});
    if dry {
        out["dryRun"] = json!(true);
        out["wouldSelect"] = json!(ids);
    }
    if let Some((ids, matched)) = found {
        out["matched"] = json!(matched);
        out["ids"] = json!(ids);
        if nothing_found {
            out["note"] = json!("nothing matched `where` or `list`; the selection is unchanged");
        }
    }
    Ok(ToolResult::json(&out))
}

/// What is selected now: one `document.inspect` for the page and the selected ids, one `node.get`
/// each (a selection is cut at [`MAX_SELECTION_LISTED`] layers).
fn get_selection(b: &mut dyn Backend) -> Result<ToolResult, String> {
    let doc = b.call("document.inspect", json!({"depth": 0, "fields": ["name"]}))?;
    let ids: Vec<u64> = doc.get("selection").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    let current = doc.get("currentPage").cloned().unwrap_or(Value::Null);
    let name = doc
        .get("pages")
        .and_then(Value::as_array)
        .and_then(|p| p.iter().find(|p| p.get("id") == Some(&current)))
        .and_then(|p| p.get("name"))
        .cloned();
    let fields = ["id", "name", "type", "parent", "x", "y", "width", "height"];
    let mut listed = Vec::new();
    for id in ids.iter().take(MAX_SELECTION_LISTED) {
        let n = b.call("node.get", json!({"id": id, "depth": 0, "fields": fields}))?;
        // Keep the listed keys only (a node.get at depth 0 also reports `childCount`).
        listed.push(Value::Object(fields.iter().filter_map(|k| n.get(*k).map(|v| ((*k).to_string(), v.clone()))).collect()));
    }
    let mut out = json!({"page": {"id": current, "name": name}, "selection": listed, "count": ids.len()});
    if ids.len() > MAX_SELECTION_LISTED {
        out["truncated"] = json!(format!("only the first {MAX_SELECTION_LISTED} of {} selected layers are listed", ids.len()));
    }
    Ok(ToolResult::json(&out))
}

/// Most selected layers `get_selection` lists with their details.
const MAX_SELECTION_LISTED: usize = 200;

/// `node.find` as a tool: reads only (it never selects), whatever the filters.
fn find_nodes(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = a.clone();
    params.retain(|_, v| !v.is_null());
    exec(b, "node.find", Value::Object(params)).map(|v| ToolResult::json(&v))
}

/// Most layers `get_nodes` reads in one call.
const MAX_NODES_READ: usize = 200;

/// What `get_nodes` reads of each layer unless `fields` says otherwise.
const DEFAULT_NODE_FIELDS: [&str; 6] = ["name", "type", "x", "y", "width", "height"];

/// A cheap outline of many layers: one `node.get` each (the fields asked for, children to `depth`).
fn get_nodes(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let limit = n_arg(a, "limit")?.map_or(MAX_NODES_READ, |v| v.clamp(1.0, MAX_NODES_READ as f64) as usize);
    let depth = n_arg(a, "depth")?.unwrap_or(1.0).clamp(0.0, 6.0) as u64;
    let fields = match a.get("fields") {
        None | Some(Value::Null) => json!(DEFAULT_NODE_FIELDS),
        Some(f) => f.clone(),
    };
    figcraft_engine::inspect::InspectOptions::parse(&json!({"fields": fields}))?;
    let (ids, total) = targets(b, a, limit)?;
    let (_, budget) = budget(a)?;
    let mut nodes: Vec<Value> = Vec::new();
    let (mut used, mut too_big) = (0usize, false);
    for id in &ids {
        let mut n = read_node(b, *id, depth, &fields);
        // Children listed in full carry no `childCount`: say it anyway, as the tool promises.
        if let Some(count) = n.get("children").and_then(Value::as_array).map(Vec::len).filter(|_| n.get("childCount").is_none()) {
            n["childCount"] = json!(count);
        }
        let mut size = wire_size(&n);
        if size + MARKER_RESERVE > budget {
            // One layer alone fills the message: its own fields and childCount, no children.
            n = read_node(b, *id, 0, &fields);
            size = wire_size(&n);
            n["truncated"] = json!("children left out: the subtree is too large for one message; read it with get_node");
        }
        if !nodes.is_empty() && used + size + MARKER_RESERVE > budget {
            too_big = true;
            break;
        }
        used += size;
        nodes.push(n);
    }
    let mut out = json!({"count": nodes.len(), "total": total, "nodes": nodes});
    if (nodes.len() as u64) < total {
        let reason = if too_big { "the response would pass what one message can carry" } else { "more layers than `limit` (at most 200)" };
        out["truncated"] =
            json!({"returned": nodes.len(), "total": total, "reason": reason, "hint": "narrow `where` or pass the rest of the ids in another call"});
    }
    Ok(ToolResult::json(&out))
}

/// `node.get` for `get_nodes`: the layer, or `{id, error}` when it can't be read.
fn read_node(b: &mut dyn Backend, id: u64, depth: u64, fields: &Value) -> Value {
    match b.call("node.get", json!({"id": id, "depth": depth, "fields": fields})) {
        Ok(Value::Null) => json!({"id": id, "error": format!("no such layer {id}")}),
        Ok(n) => n,
        Err(e) => json!({"id": id, "error": e}),
    }
}

/// Many layers on one PNG with their index numbers, and a legend that maps index to layer.
fn contact_sheet(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let scale = n_arg(a, "scale")?.unwrap_or(0.25).clamp(0.02, 1.0);
    let columns = n_arg(a, "columns")?.unwrap_or(4.0).clamp(1.0, 16.0) as usize;
    let max_size = n_arg(a, "maxSize")?.unwrap_or(2400.0).clamp(64.0, 8192.0) as u32;
    let (ids, total) = targets(b, a, crate::sheet::MAX_FRAMES)?;
    if ids.is_empty() {
        return Err("no layers to draw: `ids`, `where` or `list` named none".into());
    }
    let cols = columns.min(ids.len());
    // A frame larger than its share of the sheet is rendered smaller at once, not rendered whole.
    let cell = f64::from(max_size) / cols as f64;
    let (mut frames, mut legend, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for id in &ids {
        match render_frame(b, *id, scale, cell) {
            Ok((pix, entry)) => {
                frames.push(pix);
                legend.push(json!({"index": frames.len(), "id": id, "name": entry["name"], "width": entry["width"], "height": entry["height"]}));
            }
            Err(e) => failed.push(json!({"id": id, "error": e})),
        }
    }
    if frames.is_empty() {
        return Err(format!("no layer could be drawn: {}", Value::Array(failed)));
    }
    let bytes = figcraft_render::encode_png(&crate::sheet::compose(&frames, cols, max_size)?)?;
    let (w, h) = png_info(&bytes);
    let mut info = json!({"frames": legend, "columns": cols, "scale": scale, "width": w, "height": h});
    if (ids.len() as u64) < total {
        info["truncated"] = json!(format!("only the first {} of {total} layers are drawn", ids.len()));
    }
    if !failed.is_empty() {
        info["failed"] = Value::Array(failed);
    }
    Ok(ToolResult::image(&bytes, "image/png", &info))
}

/// One layer rendered at `scale`, or smaller so that its longer side fits `cell` pixels; with its
/// `{name, width, height}`.
fn render_frame(b: &mut dyn Backend, id: u64, scale: f64, cell: f64) -> Result<(figcraft_render::Pixmap, Value), String> {
    let n = b.call("node.get", json!({"id": id, "depth": 0, "fields": ["name", "width", "height", "absoluteBounds"]}))?;
    if n.is_null() {
        return Err(format!("no such layer {id}"));
    }
    let side = |k: &str| n.pointer(&format!("/absoluteBounds/{k}")).or_else(|| n.get(k)).and_then(Value::as_f64).unwrap_or(0.0);
    let longer = side("width").max(side("height"));
    let s = if longer > 0.0 { scale.min(cell / longer) } else { scale };
    let png = b.render_png(&[id], s)?;
    let pix = figcraft_render::images::decode(&png).ok_or("the rendered image can't be read")?;
    Ok((pix, n))
}

/// Which backend answers, and what the engine says about each open file.
fn diagnostics(b: &mut dyn Backend) -> Result<ToolResult, String> {
    let status = exec(b, "doc.status", json!({}))?;
    let documents: Vec<Value> = status
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|d| {
            let g = |k: &str| d.get(k).cloned().unwrap_or(Value::Null);
            let saved = d
                .get("lastSavedMs")
                .and_then(Value::as_i64)
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            json!({
                "index": g("index"), "id": g("id"), "name": g("name"), "path": g("path"),
                "dirty": g("dirty"), "changedOnDisk": g("changedOnDisk"), "lastSaved": saved, "active": g("active"),
            })
        })
        .collect();
    let auto_save = exec(b, "pref.autoSave", json!({})).ok().and_then(|v| v.get("on").and_then(Value::as_bool));
    Ok(ToolResult::json(&json!({
        "backend": b.describe(),
        "attached": b.has_ui(),
        "mcpServer": {"pid": std::process::id()},
        "target": b.target(),
        "autoSave": auto_save,
        "documents": documents,
    })))
}

/// `use_document`: the file later calls act on when they name none (`null`: the one in front).
fn use_document(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let target = match a.get("document") {
        None => return Err("`document` is required: an id, a path or a name, or null for the file in front".into()),
        Some(Value::Null) => None,
        Some(d) if valid_document(d) => {
            let row = status_row(b, Some(d)).ok_or_else(|| format!("no open document matches {d} (the ids are in doc.list)"))?;
            row.get("id").cloned()
        }
        Some(_) => return Err("`document` must be an id, a path or a name, or null".into()),
    };
    b.set_target(target.clone());
    let documents = exec(b, "doc.list", json!({}))?;
    Ok(ToolResult::json(&json!({"target": target, "documents": documents})))
}

/// `copy_between_documents`: `edit.copy` in `from`, `edit.paste` in `to`; neither tab changes.
fn copy_between_documents(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let doc_of = |k: &str| match a.get(k) {
        Some(d) if valid_document(d) => Ok(d.clone()),
        _ => Err(format!("`{k}` is required: a document id (doc.list) or path")),
    };
    let (from, to) = (doc_of("from")?, doc_of("to")?);
    let at = match (n_arg(a, "x")?, n_arg(a, "y")?) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, None) => None,
        _ => return Err("pass `x` and `y` together".into()),
    };
    let mut source = InDocument { inner: &mut *b, document: from.clone() };
    let (ids, total) = targets(&mut source, a, MAX_NODES_READ)?;
    if ids.is_empty() {
        return Err("nothing to copy: `ids` or `where` named no layer".into());
    }
    if total > ids.len() as u64 {
        return Err(format!("{total} layers match `where`, at most {} are copied at once: narrow the filters", ids.len()));
    }
    // In place unless told where: the paste would otherwise centre them or follow `to`'s selection.
    let at = at.or_else(|| bounds_of(&mut source, &ids).map(|(x, y, _, _)| (x, y)));
    let copied = exec(&mut source, "edit.copy", json!({"ids": ids}))?;
    let mut dest = InDocument { inner: &mut *b, document: to.clone() };
    let parent = match a.get("parent").filter(|p| !p.is_null()) {
        Some(p) => p.clone(),
        None => dest.call("document.inspect", json!({"depth": 0}))?.get("currentPage").cloned().ok_or("the document `to` reports no current page")?,
    };
    let mut paste = json!({"keepSelection": true, "parent": parent});
    if let Some((x, y)) = at {
        paste["x"] = json!(x);
        paste["y"] = json!(y);
    }
    let pasted = exec(&mut dest, "edit.paste", paste)?;
    let new_ids = pasted.get("ids").cloned().unwrap_or_else(|| json!([]));
    let count = copied.get("copied").and_then(Value::as_u64).unwrap_or(ids.len() as u64);
    Ok(ToolResult::json(&json!({"copied": count, "newIds": new_ids, "from": from, "to": to})))
}

/// `checkpoint`: a named version of the document as it is now.
fn checkpoint(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = json!({});
    for k in ["name", "description"] {
        if let Some(s) = s_arg(a, k)? {
            params[k] = json!(s);
        }
    }
    exec(b, "file.saveVersion", params).map(|v| ToolResult::json(&v))
}

/// Most versions `list_versions` returns (a file keeps at most 100).
const MAX_VERSIONS_LISTED: usize = 100;

/// `list_versions`: the newest `limit` versions, oldest first.
fn list_versions(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let limit = n_arg(a, "limit")?.map_or(30, |v| v.clamp(1.0, MAX_VERSIONS_LISTED as f64) as usize);
    let all = exec(b, "version.list", json!({}))?;
    let all = all.as_array().map(Vec::as_slice).unwrap_or_default();
    let shown = all.get(all.len().saturating_sub(limit)..).unwrap_or_default();
    Ok(ToolResult::json(&json!({"total": all.len(), "versions": shown})))
}

/// `restore_version`: replaces the document with a version; `confirm: true` is required.
fn restore_version(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let id = req_s(a, "id")?;
    if !flag(a, "confirm")? {
        return Err(format!(
            "restore_version replaces the document with version `{id}`: pass `confirm: true`. The engine first keeps the current state as an Autosave version (unless a version of it was just saved or restored), and the restore is one undo step"
        ));
    }
    exec(b, "version.restore", json!({"id": id})).map(|v| ToolResult::json(&v))
}

/// `get_code_connect_map`: one layer's link and snippet, or every link in the file.
fn get_code_connect_map(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    if a.get("id").is_some_and(|v| !v.is_null()) {
        return exec(b, "inspect.componentCode", pick(a, &["id", "language"])).map(|v| ToolResult::json(&v));
    }
    // The export is a file's text: {version, links: [...]}.
    let exported = exec(b, "codeConnect.export", json!({}))?;
    let mut all = exported.get("text").and_then(Value::as_str).and_then(|t| serde_json::from_str::<Value>(t).ok()).unwrap_or(exported);
    if let Some(lang) = s_arg(a, "language")?
        && let Some(links) = all.get_mut("links").and_then(Value::as_array_mut)
    {
        links.retain(|l| l.get("language").and_then(Value::as_str).is_some_and(|x| x.eq_ignore_ascii_case(lang)));
    }
    Ok(ToolResult::json(&all))
}

/// `find_similar`: `family.find` with the arguments the engine validates.
fn find_similar(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let params = pick(a, &["id", "minScore", "limit"]);
    exec(b, "family.find", params).map(|v| ToolResult::json(&v))
}

/// `propagate_change`: `family.propagate`, a preview unless `confirm` is true.
fn propagate_change(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = pick(a, &["from", "ids", "props", "minScore", "deep"]);
    let confirm = flag(a, "confirm")?;
    if !confirm {
        params["dryRun"] = json!(true);
    }
    let mut out = exec(b, "family.propagate", params)?;
    if !confirm {
        out = json!({"preview": true, "note": "nothing was changed; pass confirm: true to apply", "result": out});
    }
    Ok(ToolResult::json(&out))
}

/// The listed keys of `a` that are set, as a params object.
fn pick(a: &Map<String, Value>, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for k in keys {
        if let Some(v) = a.get(*k).filter(|v| !v.is_null()) {
            out.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(out)
}

/// `compare_version`: what changed since a version, in one top-level frame or section or in all
/// of the current page's.
fn compare_version(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let version = req_s(a, "id")?;
    if let Some(node) = a.get("node").filter(|n| !n.is_null()) {
        let id = id_arg(node)?;
        return exec(b, "dev.compare", json!({"id": id, "version": version})).map(|v| ToolResult::json(&v));
    }
    let doc = b.call("document.inspect", json!({"depth": 1, "fields": ["type"]}))?;
    let current = doc.get("currentPage").cloned().unwrap_or(Value::Null);
    let pages = doc.get("pages").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let page = pages.iter().find(|p| p.get("id") == Some(&current)).or(pages.first());
    let frames: Vec<u64> = page
        .and_then(|p| p.get("children"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|c| matches!(c.get("type").and_then(Value::as_str), Some("FRAME" | "SECTION")))
        .filter_map(|c| c.get("id").and_then(Value::as_u64))
        .take(MAX_COMPARED)
        .collect();
    let (mut changed, mut unchanged, mut failed, mut against) = (Vec::new(), Vec::new(), Vec::new(), Value::Null);
    for id in &frames {
        match exec(b, "dev.compare", json!({"id": id, "version": version})) {
            Ok(mut r) => {
                against = r.get("version").cloned().unwrap_or(Value::Null);
                if r.get("changes").and_then(Value::as_array).is_some_and(|c| !c.is_empty()) {
                    changed.push(r);
                } else {
                    unchanged.push(r.get_mut("id").map(Value::take).unwrap_or(Value::Null));
                }
            }
            Err(e) => failed.push(json!({"id": id, "error": e})),
        }
    }
    let mut out = json!({"version": against, "frames": frames.len(), "changed": changed, "unchanged": unchanged});
    if !failed.is_empty() {
        out["failed"] = json!(failed);
    }
    if failed.len() == frames.len() && !frames.is_empty() {
        return Err(format!("dev.compare failed for every frame: {}", failed.first().map(|f| f["error"].to_string()).unwrap_or_default()));
    }
    Ok(ToolResult::json(&out))
}

/// Most top-level frames `compare_version` compares at once.
const MAX_COMPARED: usize = 50;

fn lint_icons(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = a.clone();
    params.retain(|_, v| !v.is_null());
    if let Some(ids) = ids_arg(a, "ids")? {
        params.insert("ids".into(), json!(ids));
    }
    exec(b, "inspect.lintIcons", Value::Object(params)).map(|v| ToolResult::json(&v))
}

fn export_source(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = Map::new();
    if let Some(p) = s_arg(a, "path")? {
        params.insert("path".into(), json!(p));
    }
    exec(b, "file.exportSource", Value::Object(params)).map(|v| ToolResult::json(&v))
}

fn export_tokens(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = a.clone();
    params.retain(|_, v| !v.is_null());
    let r = exec(b, "variable.exportTokens", Value::Object(params))?;
    // css and json come back as `text`: show it as such, not escaped inside JSON.
    Ok(match (r.get("text").and_then(Value::as_str), r.get("theme")) {
        (Some(t), None) => ToolResult::text(t.to_string()),
        _ => ToolResult::json(&r),
    })
}

fn check_contrast(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = a.clone();
    params.retain(|_, v| !v.is_null());
    if let Some(ids) = ids_arg(a, "ids")? {
        params.insert("ids".into(), json!(ids));
    }
    let pair = params.contains_key("fg") || params.contains_key("bg");
    exec(b, if pair { "a11y.contrast" } else { "a11y.checkContrast" }, Value::Object(params)).map(|v| ToolResult::json(&v))
}

/// The colour-vision simulation `a` asks for (`cvd`), checked before anything renders.
fn cvd_arg(a: &Map<String, Value>) -> Result<Option<figcraft_render::cvd::Cvd>, String> {
    s_arg(a, "cvd")?
        .map(|s| figcraft_render::cvd::Cvd::parse(s).ok_or_else(|| format!("unknown cvd `{s}` (protanopia|deuteranopia|tritanopia|achromatopsia)")))
        .transpose()
}

fn history(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let v = if let Some(i) = n_arg(a, "revertTo")? {
        exec(b, "history.revertTo", json!({"index": i}))?
    } else if a.get("hash").and_then(Value::as_bool) == Some(true) {
        exec(b, "history.hash", json!({}))?
    } else {
        let mut params = a.clone();
        params.retain(|k, v| k == "limit" && !v.is_null());
        exec(b, "history.list", Value::Object(params))?
    };
    Ok(ToolResult::json(&v))
}

fn auto_name_layers(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = a.clone();
    params.retain(|_, v| !v.is_null());
    if let Some(ids) = ids_arg(a, "ids")? {
        params.insert("ids".into(), json!(ids));
    }
    exec(b, "layer.autoName", Value::Object(params)).map(|v| ToolResult::json(&v))
}

fn export_many(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = a.clone();
    params.retain(|_, v| !v.is_null());
    if let Some(ids) = ids_arg(a, "ids")? {
        params.insert("ids".into(), json!(ids));
    }
    exec(b, "file.exportMany", Value::Object(params)).map(|v| ToolResult::json(&v))
}

fn export(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = Map::new();
    if let Some(ids) = ids_arg(a, "ids")? {
        params.insert("ids".into(), json!(ids));
    }
    let format = s_arg(a, "format")?.map(str::to_ascii_lowercase);
    if let Some(f) = &format {
        if !["png", "jpg", "jpeg", "svg", "webp", "pdf", "xml", "fig"].contains(&f.as_str()) {
            return Err(format!("unknown format `{f}` (png|jpg|svg|webp|pdf|xml|fig)"));
        }
        params.insert("format".into(), json!(f));
    }
    if let Some(s) = n_arg(a, "scale")? {
        if s <= 0.0 {
            return Err("`scale` must be positive".into());
        }
        params.insert("scale".into(), json!(s));
    }
    if let Some(s) = s_arg(a, "size")? {
        params.insert("size".into(), json!(s));
    }
    // PDF print options and the export options, checked by the command.
    for k in [
        "bleed",
        "cropMarks",
        "bleedMarks",
        "cmyk",
        "contentsOnly",
        "includeBoundingBox",
        "svgOutlineText",
        "svgIdAttribute",
        "svgSimplifyStroke",
        "iconPreset",
        "currentColor",
        "vdCompat",
        "cvd",
    ] {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            params.insert(k.into(), v.clone());
        }
    }
    if let Some(p) = s_arg(a, "path")? {
        params.insert("path".into(), json!(p));
        return exec(b, "file.export", Value::Object(params)).map(|v| ToolResult::json(&v));
    }
    let r = exec(b, "file.export", Value::Object(params))?;
    let fmt = r.get("format").and_then(Value::as_str).unwrap_or("png").to_string();
    let bytes = decode_data(&r)?;
    match fmt.as_str() {
        "png" => {
            let (w, h) = png_info(&bytes);
            Ok(ToolResult::image(&bytes, "image/png", &json!({"format": "png", "width": w, "height": h, "bytes": bytes.len()})))
        }
        "jpg" | "jpeg" => Ok(ToolResult::image(&bytes, "image/jpeg", &json!({"format": "jpg", "bytes": bytes.len()}))),
        "webp" => Ok(ToolResult::image(&bytes, "image/webp", &json!({"format": "webp", "bytes": bytes.len()}))),
        "svg" => Ok(ToolResult::text(String::from_utf8_lossy(&bytes).into_owned())),
        "xml" => {
            let info = json!({"format": "xml", "bytes": bytes.len(), "warnings": r.get("warnings").cloned().unwrap_or_else(|| json!([]))});
            let mut out = ToolResult::text(String::from_utf8_lossy(&bytes).into_owned());
            out.content.extend(ToolResult::json(&info).content);
            Ok(out)
        }
        other => {
            let mut out = json!({"format": other, "bytes": bytes.len(), "data": base64::engine::general_purpose::STANDARD.encode(&bytes)});
            if let Some(w) = r.get("warnings") {
                out["warnings"] = w.clone();
            }
            Ok(ToolResult::json(&out))
        }
    }
}

fn screenshot(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let cvd = cvd_arg(a)?;
    let tint = |bytes: Vec<u8>| match cvd {
        Some(c) => figcraft_render::cvd::filter_png(&bytes, c),
        None => Ok(bytes),
    };
    let id = a.get("window").and_then(Value::as_u64);
    if (id.is_some() || matches!(a.get("window"), Some(Value::Bool(true)))) && b.has_ui() {
        let bytes = match id {
            Some(id) => b.window_png(id)?,
            None => b.screenshot_png()?,
        };
        let bytes = tint(bytes)?;
        let (w, h) = png_info(&bytes);
        return Ok(ToolResult::image(&bytes, "image/png", &json!({"source": "window", "width": w, "height": h})));
    }
    let max = n_arg(a, "maxSize")?.unwrap_or(1600.0).clamp(16.0, 8192.0);
    let req = n_arg(a, "scale")?.unwrap_or(1.0);
    if req <= 0.0 {
        return Err("`scale` must be positive".into());
    }
    let ids = ids_arg(a, "ids")?.unwrap_or_default();
    let measured = if ids.is_empty() { page_children(b)? } else { ids.clone() };
    let mut scale = req;
    if let Some((x0, y0, x1, y1)) = bounds_of(b, &measured) {
        let side = (x1 - x0).max(y1 - y0).max(1e-6);
        scale = scale.min(max / side);
    }
    let mut bytes = b.render_png(&ids, scale)?;
    let (mut w, mut h) = png_info(&bytes);
    // Layer bounds leave out effects (shadows) and overflow of unclipped frames, but the render
    // includes them: shrink to the measured image until it fits `maxSize` (rounding can add a pixel).
    for _ in 0..4 {
        let side = f64::from(w.max(h));
        if side <= max {
            break;
        }
        scale *= max / side;
        bytes = b.render_png(&ids, scale)?;
        (w, h) = png_info(&bytes);
    }
    let mut bytes = tint(bytes)?;
    let mut info = json!({"source": if ids.is_empty() { "page" } else { "layers" }, "ids": ids, "scale": scale, "width": w, "height": h, "cvd": cvd.map(figcraft_render::cvd::Cvd::id)});
    if let Some((marked, legend)) = mark_layers(b, a, &measured, scale, (w, h), &bytes)? {
        bytes = marked;
        info["marks"] = legend;
    }
    Ok(ToolResult::image(&bytes, "image/png", &info))
}

/// The `marks` of `screenshot`: the PNG with badges, and the legend `{style, badges, truncated?}`.
/// The render is centred on the union of the `measured` layers' bounds, which maps page to pixels.
fn mark_layers(
    b: &mut dyn Backend,
    a: &Map<String, Value>,
    measured: &[u64],
    scale: f64,
    size: (u32, u32),
    png: &[u8],
) -> Result<Option<(Vec<u8>, Value)>, String> {
    let opts = match a.get("marks") {
        None | Some(Value::Null | Value::Bool(false)) => return Ok(None),
        Some(Value::Bool(true)) => Map::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => return Err("`marks` must be true or an object {ids?, style?, limit?}".into()),
    };
    let by_id = match s_arg(&opts, "style")? {
        None | Some("index") => false,
        Some("id") => true,
        Some(other) => return Err(format!("marks.style must be \"index\" or \"id\", not \"{other}\"")),
    };
    let limit = n_arg(&opts, "limit")?.unwrap_or(crate::sheet::MAX_MARKS as f64).clamp(1.0, 200.0) as usize;
    let depth = n_arg(a, "markDepth")?.unwrap_or(1.0).clamp(0.0, 5.0) as u64;
    let roots = ids_arg(&opts, "ids")?.unwrap_or_else(|| measured.to_vec());
    let (bx0, by0, bx1, by1) = bounds_of(b, measured).ok_or("marks: nothing to mark")?;
    let ox = (f64::from(size.0) - (bx1 - bx0) * scale) / 2.0;
    let oy = (f64::from(size.1) - (by1 - by0) * scale) / 2.0;
    // Breadth first: the top-level layers, then their children, so the cap drops the deepest first.
    let mut queue: std::collections::VecDeque<(u64, u64)> = roots.iter().map(|i| (*i, 0)).collect();
    let (mut labels, mut at, mut legend, mut truncated, mut visited) = (Vec::new(), Vec::new(), Vec::new(), false, 0usize);
    while let Some((id, d)) = queue.pop_front() {
        visited += 1;
        if visited > 4 * limit + 50 {
            truncated = true;
            break;
        }
        let Ok(n) = b.call("node.get", json!({"id": id, "depth": 1})) else { continue };
        if n.get("visible") == Some(&Value::Bool(false)) {
            continue;
        }
        if d < depth {
            for cid in n.get("children").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.get("id").and_then(Value::as_u64)) {
                queue.push_back((cid, d + 1));
            }
        }
        let ab = n.get("absoluteBounds");
        let g = |k: &str| ab.and_then(|v| v.get(k)).and_then(Value::as_f64);
        let (Some(x), Some(y), Some(w), Some(h)) = (g("x"), g("y"), g("width"), g("height")) else { continue };
        let (px, py) = (ox + (x - bx0) * scale, oy + (y - by0) * scale);
        if px + w * scale < 0.0 || py + h * scale < 0.0 || px > f64::from(size.0) || py > f64::from(size.1) {
            continue;
        }
        if labels.len() >= limit {
            truncated = true;
            break;
        }
        let badge = if by_id { id } else { labels.len() as u64 + 1 };
        labels.push(badge);
        at.push((px.floor() as i64, py.floor() as i64));
        legend.push(json!({"badge": badge, "id": id, "name": n.get("name").cloned().unwrap_or(Value::Null)}));
    }
    let mut pix = figcraft_render::images::decode(png).ok_or("the rendered image can't be read")?;
    crate::sheet::draw_marks(&mut pix, &labels, &at);
    let bytes = figcraft_render::encode_png(&pix)?;
    let mut out = json!({"style": if by_id { "id" } else { "index" }, "badges": legend});
    if truncated {
        out["truncated"] = json!(format!("only the first {} layers are marked: raise marks.limit or name fewer `ids`", labels.len()));
    }
    Ok(Some((bytes, out)))
}

fn open_file(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let path = req_s(a, "path")?;
    let mut params = json!({"path": path});
    match a.get("variables") {
        None | Some(Value::Null) => {}
        Some(v @ (Value::String(_) | Value::Object(_))) => params["variables"] = v.clone(),
        Some(_) => return Err("`variables` must be a path or the local variables JSON object".into()),
    }
    if let Some(reload) = a.get("reload").filter(|v| !v.is_null()) {
        params["reload"] = reload.clone();
    }
    // With a window the file opens behind the user's tab, which stays where it is, unless `focus`.
    let focus = flag(a, "focus")?;
    if b.has_ui() && !focus {
        params["activate"] = json!(false);
    }
    let r = exec(b, "file.open", params)?;
    let id = r.get("id").filter(|i| valid_document(i)).cloned().filter(|_| b.has_ui());
    // The agent keeps working on what it opened, wherever the user clicks next.
    b.set_target(id.clone());
    let Some(id) = id else {
        let doc = b.call("document.inspect", json!({"depth": 1}))?;
        return Ok(ToolResult::json(&json!({"opened": r, "document": doc})));
    };
    let doc = InDocument { inner: &mut *b, document: id.clone() }.call("document.inspect", json!({"depth": 1}))?;
    let mut out = json!({"opened": r, "document": doc});
    if !focus {
        out["note"] = json!(format!("opened in the background as document {id}; later calls act on it (pass `document` to act on another tab)"));
    }
    Ok(ToolResult::json(&out))
}

fn save_file(b: &mut dyn Backend, a: &Map<String, Value>, document: Option<&Value>) -> Result<ToolResult, String> {
    let params = match s_arg(a, "path")? {
        Some(p) => json!({"path": p}),
        None => json!({}),
    };
    // The state before the save: in the app's window `dirty` means the person has unsaved changes
    // that this save writes too, and `changedOnDisk` that the file on disk is not the one it read.
    let before = status_row(b, document);
    let mut out = exec(b, "file.save", params)?;
    if let Some(row) = before {
        let mut extra = Map::new();
        if b.has_ui() && row.get("dirty") == Some(&Value::Bool(true)) {
            extra.insert("unsavedBefore".into(), json!(true));
            extra.insert("unsavedNote".into(), json!("the window had unsaved changes before this save: they are written too"));
        }
        if row.get("changedOnDisk") == Some(&Value::Bool(true)) {
            extra.insert("changedOnDisk".into(), json!(true));
        }
        if !extra.is_empty() {
            if !out.is_object() {
                out = json!({"result": out});
            }
            if let Some(o) = out.as_object_mut() {
                o.extend(extra);
            }
        }
    }
    Ok(ToolResult::json(&out))
}

/// The `doc.status` row of the document a tool acts on: the one `document` names (id, path or
/// name), else the one in front. `None` when the status is unavailable.
fn status_row(b: &mut dyn Backend, document: Option<&Value>) -> Option<Value> {
    let rows = exec(b, "doc.status", json!({})).ok()?;
    let rows = rows.as_array()?;
    let named = |r: &&Value| match document {
        None => r.get("active") == Some(&Value::Bool(true)),
        Some(Value::String(s)) => {
            r.get("path").and_then(Value::as_str) == Some(s.as_str()) || r.get("name").and_then(Value::as_str) == Some(s.as_str())
        }
        Some(d) => d.as_u64().is_some() && r.get("id").and_then(Value::as_u64) == d.as_u64(),
    };
    rows.iter().find(named).cloned()
}

fn mods(a: &Map<String, Value>, out: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["shift", "alt", "cmd", "ctrl"] {
        match a.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(x)) => {
                out.insert(k.into(), json!(x));
            }
            Some(_) => return Err(format!("`{k}` must be a boolean")),
        }
    }
    Ok(())
}

fn pointer(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let x = n_arg(a, "x")?.ok_or("`x` is required")?;
    let y = n_arg(a, "y")?.ok_or("`y` is required")?;
    let kind = s_arg(a, "kind")?.unwrap_or("click");
    if let Some(t) = s_arg(a, "tool")? {
        exec(b, "tool.set", json!({"tool": t}))?;
    }
    let mut p = Map::new();
    p.insert("kind".into(), json!(kind));
    p.insert("x".into(), json!(x));
    p.insert("y".into(), json!(y));
    if let Some(to) = a.get("to").filter(|v| !v.is_null()) {
        let arr =
            to.as_array().filter(|t| t.len() == 2 && t.iter().all(|v| figcraft_engine::params::f64_of(v).is_some())).ok_or("`to` must be [x, y]")?;
        p.insert("to".into(), Value::Array(arr.clone()));
    }
    if let Some(s) = n_arg(a, "steps")? {
        p.insert("steps".into(), json!(s.clamp(1.0, 200.0) as u64));
    }
    for k in ["pressure", "pressureTo"] {
        if let Some(v) = n_arg(a, k)? {
            p.insert(k.into(), json!(v.clamp(0.0, 1.0)));
        }
    }
    mods(a, &mut p)?;
    b.call("ui.pointer", Value::Object(p)).map(|v| ToolResult::json(&v))
}

fn key(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let k = req_s(a, "key")?;
    let mut p = Map::new();
    p.insert("key".into(), json!(k));
    mods(a, &mut p)?;
    b.call("ui.key", Value::Object(p)).map(|v| ToolResult::json(&v))
}

fn type_text(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let text = req_s(a, "text")?;
    match a.get("id").filter(|v| !v.is_null()) {
        Some(id) => {
            let id = id_arg(id)?;
            exec(b, "text.edit.begin", json!({"id": id}))?;
            let typed = b.call("ui.type", json!({"text": text}));
            // Always leave edit mode, even when typing failed.
            let ended = exec(b, "text.edit.end", json!({}));
            typed?;
            ended?;
            let node = b.call("node.get", json!({"id": id, "depth": 0}))?;
            Ok(ToolResult::json(&json!({"ok": true, "node": node})))
        }
        None => b.call("ui.type", json!({"text": text})).map(|v| ToolResult::json(&v)),
    }
}
