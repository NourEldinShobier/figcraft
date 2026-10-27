# MCP design notes: not built yet

Two ideas from the MCP gap evaluation need a larger design than a command. Each is sketched here so the next person starts from a decision.

## Stable ids across save, open and `.fig` import

Layer ids are small integers handed out per document, and a `.fig` import (and a re-import of a saved file) renumbers them, so a script that holds ids loses them across a round trip. The plan is an optional `key` string on a layer, set by `create_node {key}` or `node.set {key}` and unique within the file, stored in the `.figcraft` file and written to the `.fig` file as plugin data under a FigCraft namespace so it survives the format. Every command that takes an id would also take `{"key": "card/title"}`, resolved in `crates/engine/src/params` next to `{"$ref"}`, and `batch` could use keys in place of `as` names across calls. This touches the document model, the file formats and every id parameter parser (crates/doc, crates/format), so it should land as one change with a sweep over all commands, not piecemeal.

## Inline scripting in a batch

Agents currently build loops by generating many `batch` steps. The WASM plugin host already runs sandboxed code with a metered budget and rollback, so the cheapest route is a `script.run {code | pluginId, input}` command that runs one plugin call as a single undo step with no network access and returns its value, rather than a new expression language inside `batch`. A small `for_each` over an earlier step's `ids` in `batch` itself (a step with `{"each": {"$ref": "icons", "field": "ids"}, "command": ..., "params": {"ids": ["$item"]}}`) would cover most loops without code. The open questions are the fuel limit, what the sandbox may read (the document only), and whether the Rust-only rule for the repo allows shipping a documented JS or Lua runtime; the plugin SDK is Rust compiled to WASM today, which agents cannot compile on the fly.