# Plain-text design source

A canonical, diffable text form of a FigCraft document, for version control, code review and
scripts. Written by `file.exportSource` (MCP tool `export_source`, CLI
`figcraft-cli export <file> -o <out> --format source`) and read back by `file.importSource`.

## Guarantees

- Saving the same document twice gives byte-identical text.
- Changing one layer's fill changes exactly one line.
- `source -> import -> source` is the identity.
- No volatile data: no save time, no live-session state (countdown timer, vote, ballots). Comment
  times are design data entered by users and do not change on save.

## Layout

JSON Lines (UTF-8, `\n`, one object per line, every object's keys sorted, numbers as JSON writes
them). A line is one of:

| Line | Meaning |
|---|---|
| `{"sourceVersion":1,"t":"doc","v":{...}}` | First line. The document's scalar fields (`name`, `format`, `version`, the next layer id, the current page, empty lists…). |
| `{"t":"<field>","v":<item>}` | One item of a non-empty list field of the document (`pages`, `styles`, `comments`, `imageIndex`, …), in list order. |
| `{"t":"<field>","k":"<key>","v":<value>}` | One entry of a non-empty map field (e.g. `variables`), sorted by key. |
| `{"t":"<field>.<sub>","v":<item>}` | One item of a list inside a map field (e.g. `variables.variables`). |
| `{"t":"node","v":{...}}` | One layer, with every property the `.figcraft` file holds except `children`. |

Layer lines come last, in tree order: pages in page order, each layer directly before its
children, children bottom-to-top as in the file. `children` is rebuilt on import from each layer's
`parent` and the line order.

## Identity

- Layers keep their numeric `id` (and `parent`). Ids do not change when other layers are edited, so
  an edit stays on the edited layer's line. They are not replaced by paths: a rename or move would
  then rewrite every line below it.
- A layer's **key** (`node.set {props: {key}}`) appears inside the layer line as plugin data
  `"figcraft.key"`. It is the stable name to use across files.
- Images are referenced by SHA-256 hash (`imageIndex` lines and image paints). With a folder
  output (`-o dir`, no file extension) the bytes are written to `images/<hash>` beside
  `design.jsonl`; with a single file they are not included and import warns about each missing one.

## Compatibility

The layer objects are the `.figcraft` file's own, so a field added to the file format appears
in the source with the same optional, additive rules (older builds ignore it). `sourceVersion` is
bumped only when the line layout changes; a newer source is refused.

## JSON Schema

Each line validates against:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://github.com/storytold/figcraft/docs/source-format.schema.json",
  "title": "FigCraft design source line",
  "type": "object",
  "required": ["t", "v"],
  "properties": {
    "t": {
      "type": "string",
      "description": "'doc' (first line), 'node' (a layer), or a document field name, optionally 'field.sub' for a list inside a map field."
    },
    "k": { "type": "string", "description": "Map entry key; only on map-field lines." },
    "sourceVersion": { "const": 1, "description": "Only on the 'doc' line." },
    "v": { "description": "'doc': object of scalar fields. 'node': the layer object. Otherwise the item or entry value." }
  },
  "additionalProperties": false,
  "allOf": [
    {
      "if": { "properties": { "t": { "const": "doc" } } },
      "then": { "required": ["sourceVersion"], "properties": { "v": { "type": "object" } } }
    },
    {
      "if": { "properties": { "t": { "const": "node" } } },
      "then": {
        "properties": {
          "v": {
            "type": "object",
            "required": ["id", "name", "type"],
            "properties": {
              "id": { "type": ["integer", "string"] },
              "parent": { "type": ["integer", "string"] },
              "name": { "type": "string" }
            },
            "not": { "required": ["children"] }
          }
        }
      }
    }
  ]
}
```

Document-level ordering: the `doc` line first, then the field lines sorted by field name, then the
`node` lines.
