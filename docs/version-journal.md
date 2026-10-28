# Version journal (`.figcraft` history storage)

Automatic versions (checkpoints) used to be stored as one full `document.json`-style snapshot each
(about 55 MB packed at 200 000 layers). The journal stores an automatic version as a delta against
the version before it. Code: `crates/format/src/journal.rs`; tests: `crates/format/src/journal_tests.rs`.

## Layout inside the zip

| Member | Content |
|---|---|
| `versions/index.json`, `versions/<id>.json` | As before, but only the **keyframes** (full snapshots). Builds that don't know the journal read these and show fewer versions. |
| `history/meta.json` | Written only when the file holds at least one delta. `{"format":1,"versions":[...]}`, oldest first; the version DAG and checksums. |
| `history/<id>.delta` | Deflated JSON delta of an automatic version against its `base`. |

`meta.json` entry: `id, name, description, time, auto?, images, kind ("key"|"delta"), parent (the
version before it in history, null for the first), base (deltas: the version it is replayed on; today
always the parent), crc (CRC-32 of the unpacked member), size (unpacked bytes)`.
Files without `history/meta.json` are read as before (all versions full). A readable `meta.json` is
authoritative for the main file's versions (`versions/index.json` is then ignored).

## Delta

```
{ "shell":  Patch,                       // document fields other than the layers (pages, styles, name, id counter ...)
  "nodes":  [ {"id", "node": {...}}      // new layer, whole
            | {"id", "patch": Patch} ],  // changed layer
  "removed": [id, ...] }
Patch = { "set": {field: value}, "absent": [field], "splice": {field: [start, removed_count, [inserted...]]} }
```

Changed layers come from `Document::changed_ids` (persistent-map diff: cost follows the edit, not the
document). A field is compared as serialized JSON; arrays of 16+ items (a frame's `children`, the
`pages` list) change as one splice, so adding a layer to a 3000-child page costs a few bytes.
Replay deserializes `base shell + patch` and the layers, with no repair pass, and reproduces
the serialized document exactly (`Document::shell`, `set_nodes_of`, `put_node`, `drop_node` in `figcraft-doc`).

## When a keyframe is written

First version; every named (manual) version; a version whose delta can't be computed; after 8 deltas
in a row (`KEYFRAME_EVERY`); after 20 MB of unpacked deltas (`LOG_BYTES`). The plan is made at save
time from the versions in the list, so removing a version (e.g. the oldest autosave making room)
just re-plans: the version after it becomes a keyframe or a delta on its new predecessor. Deltas read
from a file are copied without recompression when their base is unchanged; a new delta is computed
once per version and cached (shared by clones, as `packed` is).

The read budget (`VersionLimits`) counts keyframes at full size and deltas at delta size.

## Damaged files

Reading checks every journal member's CRC-32 against `meta.json`. A member that is missing, unreadable
or fails its checksum is dropped with a message in `Loaded::issues`, and so is every later delta built
on it (its base is missing); keyframes after it are unaffected. A broken `meta.json` is reported and the
keyframes under `versions/` are read instead. None of this fails opening the document.

## Not done yet

- Branch version histories still store full snapshots (only the main file uses the journal).
- Deltas are per layer field, not per nested property (a changed fill replaces the whole `fills` array).
- No garbage collection of images by reachability; no merge/DAG branching beyond `parent == previous`.
- Preview/restore still materializes the version on the UI thread (a chain costs one keyframe parse plus
  up to 7 delta replays).
- `file.saveVersion` and `changed_since_checkpoint` are unchanged (full compare on the UI thread).
- Layers that are equal but serialize differently (`-0.0` vs `0.0`) are treated as unchanged.
- Not tested at 200k layers (the size numbers come from a 3000-layer document).
