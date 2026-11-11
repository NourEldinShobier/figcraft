//! Tool profiles: `full` lists every tool, `core` a short list with short descriptions for agents
//! with little context. Tools outside `core` still work through `run_command` and can be listed with
//! `more_tools`.

use serde_json::{Value, json};

use crate::tools::TOOL_NAMES;

/// The env var that picks the profile when no flag does.
pub const PROFILE_ENV: &str = "FIGCRAFT_MCP_PROFILE";

/// The tools the `core` profile lists, besides `more_tools`.
pub const CORE_TOOLS: &[&str] = &[
    "list_commands",
    "run_command",
    "batch",
    "create_node",
    "set_properties",
    "get_node",
    "find_nodes",
    "get_nodes",
    "inspect_document",
    "summarize_document",
    "select",
    "screenshot",
    "export",
    "undo",
    "redo",
    "diagnostics",
    "use_document",
    "open_file",
    "save_file",
    "checkpoint",
    "restore_version",
];

/// The tool that lists the hidden tools. It is always callable; only the core profile lists it.
pub const MORE_TOOLS: &str = "more_tools";

/// How many tools and how verbose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Full,
    Core,
}

impl Profile {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "core" => Ok(Self::Core),
            other => Err(format!("unknown MCP profile `{other}` (use `core` or `full`)")),
        }
    }

    /// The flag wins over the env var; neither means `full`.
    pub fn resolve(flag: Option<&str>, env: Option<&str>) -> Result<Self, String> {
        match (flag, env) {
            (Some(f), _) => Self::parse(f),
            (None, Some(e)) if !e.trim().is_empty() => Self::parse(e).map_err(|m| format!("{PROFILE_ENV}: {m}")),
            _ => Ok(Self::Full),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Core => "core",
        }
    }

    /// Whether `tools/list` shows `tool`.
    pub fn lists(self, tool: &str) -> bool {
        self == Self::Full || CORE_TOOLS.contains(&tool) || tool == MORE_TOOLS
    }

    /// The note `tools/list` and `initialize` carry.
    pub fn note(self) -> String {
        match self {
            Self::Full => {
                "Tool profile: full (every tool is listed). `--profile core` or FIGCRAFT_MCP_PROFILE=core lists fewer, with short descriptions."
                    .into()
            }
            Self::Core => format!(
                "Tool profile: core ({} tools, short descriptions). Every other tool is hidden: more_tools lists them, and their engine commands still run through run_command (list_commands). Start the server with `--profile full` or FIGCRAFT_MCP_PROFILE=full to list all tools.",
                CORE_TOOLS.len() + 1
            ),
        }
    }

    /// The error for calling a tool this profile hides.
    pub fn hidden_error(self, tool: &str) -> String {
        format!(
            "tool `{tool}` is hidden in the {} profile. See more_tools for what is hidden; run its engine command through run_command (list_commands with a filter), or restart the server with `--profile full` (or FIGCRAFT_MCP_PROFILE=full) to use it.",
            self.name()
        )
    }
}

/// Everything up to the first sentence end (not an `e.g.`/`i.e.`), at most `max` bytes.
pub fn first_sentence(s: &str, max: usize) -> String {
    let mut end = s.len();
    for (i, _) in s.match_indices(". ") {
        let head = s.get(..i).unwrap_or("");
        if !(head.ends_with("e.g") || head.ends_with("i.e") || head.ends_with("etc")) {
            end = i + 1;
            break;
        }
    }
    let t = s.get(..end).unwrap_or(s).trim();
    if t.len() <= max {
        return t.to_string();
    }
    let mut cut = max;
    while !t.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", t.get(..cut).unwrap_or("").trim_end())
}

/// Most argument names the short description lists.
const MAX_ARGS: usize = 8;

/// A `core` entry: first sentence of the description plus the key arguments, and each property
/// description cut to its first sentence.
pub fn shorten(tool: &Value) -> Value {
    let mut t = tool.clone();
    let desc = tool.get("description").and_then(Value::as_str).unwrap_or("");
    let required: Vec<&str> =
        tool.pointer("/inputSchema/required").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut args: Vec<String> = Vec::new();
    if let Some(props) = tool.pointer("/inputSchema/properties").and_then(Value::as_object) {
        for k in props.keys().filter(|k| required.contains(&k.as_str())) {
            args.push(k.clone());
        }
        for k in props.keys().filter(|k| !required.contains(&k.as_str()) && !matches!(k.as_str(), "documents" | "ifRev")) {
            args.push(format!("{k}?"));
        }
    }
    let more = args.len() > MAX_ARGS;
    args.truncate(MAX_ARGS);
    let mut d = first_sentence(desc, 220);
    if !args.is_empty() {
        d.push_str(&format!(" Args: {}{}.", args.join(", "), if more { ", ..." } else { "" }));
    }
    t["description"] = json!(d);
    if let Some(props) = t.pointer_mut("/inputSchema/properties").and_then(Value::as_object_mut) {
        for p in props.values_mut() {
            let short = p.get("description").and_then(Value::as_str).map(|s| first_sentence(s, 140));
            if let (Some(s), Some(o)) = (short, p.as_object_mut()) {
                o.insert("description".into(), json!(s));
            }
        }
    }
    t
}

/// The `more_tools` tool definition.
fn more_tools_definition() -> Value {
    json!({
        "name": MORE_TOOLS,
        "title": "List hidden tools",
        "description": "List the tools this profile hides, one line each, and how to enable them. Args: filter?.",
        "inputSchema": {"type": "object", "properties": {"filter": {"type": "string", "description": "Case-insensitive substring of the tool name or description"}}, "required": []},
        "annotations": {"title": "List hidden tools", "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false},
    })
}

/// The `tools/list` tools for `profile`, given the full definitions.
pub fn list(profile: Profile, all: Vec<Value>) -> Vec<Value> {
    if profile == Profile::Full {
        return all;
    }
    let mut out: Vec<Value> = CORE_TOOLS.iter().filter_map(|n| all.iter().find(|t| t["name"] == *n)).map(shorten).collect();
    out.push(more_tools_definition());
    out
}

/// The `more_tools` answer: one line per hidden tool.
pub fn more_tools(profile: Profile, all: &[Value], filter: Option<&str>) -> String {
    if profile == Profile::Full {
        return "The full profile lists every tool; nothing is hidden.".into();
    }
    let f = filter.map(str::to_lowercase).filter(|f| !f.is_empty());
    let mut lines = Vec::new();
    for t in all {
        let name = t.get("name").and_then(Value::as_str).unwrap_or("");
        if profile.lists(name) || !TOOL_NAMES.contains(&name) {
            continue;
        }
        let line = format!("{name}: {}", first_sentence(t.get("description").and_then(Value::as_str).unwrap_or(""), 140));
        if f.as_ref().is_none_or(|f| line.to_lowercase().contains(f)) {
            lines.push(line);
        }
    }
    format!(
        "{} hidden tool(s){}. Use one by restarting the server with `--profile full` (or FIGCRAFT_MCP_PROFILE=full); most also work as engine commands through run_command (list_commands with a filter).\n{}",
        lines.len(),
        f.map(|f| format!(" matching `{f}`")).unwrap_or_default(),
        lines.join("\n")
    )
}
