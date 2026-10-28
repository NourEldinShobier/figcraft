# Lazy `.fig` import

## Built: first page early (page-first open)

`fig::import_staged` (format) maps pages in file order with the same mapper as `import`. The first
visible page (when more follow) is built at once into an `rest::Incremental` (the same `Ctx` the
whole import uses; its metadata is owned, so it can start before the file JSON is complete). A
copy of it, finished on its own (library copies, instances of mains on later pages stay frames), is
handed to the caller. The original goes on: later pages stay packed until the message is dropped,
then are built into the same `Ctx`, so page 1 is built once. The final document is byte-identical to
`import` (SHA-256 of latch.fig `b8ba057f...`, files-modern `35a71aaa...`, checked through the
staged path with `FIGCRAFT_FIG_STAGED=1`).

The engine (`preload_staged`, `Session::poll_loading`) opens the early document as a tab with
`DocState.loading` set: edits, save and export are refused (`LOADING_READ_ONLY`), the import report
and font check wait. When the whole file arrives the tab keeps its uid, view and page and gets
the whole document (selection restarts). The app uses `PageFirst::APP`: at least 100 000 node
changes, several pages, and a first page of at most 40% of the layers. A "Loading the other pages"
banner shows meanwhile. Wasm has no worker thread, so it keeps the eager path.

Measured on files-modern (542k layers, 4 pages, first page 275k = 51% of the layers), loaded
machine, release, engine open (`open_timing`), forced on with `PageFirst::ALWAYS`:

| | plain | page-first |
|---|---|---|
| first page shown | 98.9 s | 45.9 s |
| whole file | 101.6 s | 105.0 s (+3%) |
| peak working set | 2.27 GB | 3.16 GB (+39%) |

The memory cannot be brought to the plain open's: the first page is built while the decoded message
(about 1.7 GB here) is still held, which the plain import never does (it builds after dropping the
message). The overhead is about the first page's built size, so it grows with its share of the
layers; hence the 40% gate, which leaves files-modern on the plain path. Building the later pages
while mapping made it worse (3.6 GB), so they wait. To get within 5% of the plain peak the first
page would have to be under about 6% of the layers.

Not done (the rest of the plan below): memory is not reduced; the swap rebuilds the tab state on the
UI thread (about 1.5 s here); the first page is read-only instead of editable; instance links of the
first page are made only for the whole document.

# Lazy `.fig` import (design for the rest)

Measured on a 28 MB file (542k layers, 4 pages; release build): kiwi decode ~4 s, mapping to REST
JSON ~11 s, JSON to `Document` ~6 s, 7 GB peak. The first two stages touch every page, so the window
cannot show the active page earlier than ~15 s without changing them.

## Why it is not a small change

- The mapper (`fig/map`) shares state across pages: the layer budget (`count`), the vector network
  table (`networks`, indexed by `vectorNetworkRef`), override scopes, warnings and the component
  mains an instance names. A page mapped alone gets other indexes and counts than in the full run, so
  the "identical document" guarantee holds only if every page is mapped in file order.
- Instances reach mains on other pages and in the hidden internal canvas (`library_context`).
  `rest::import_parts` resolves instances, styles, variables and prototype links after all pages
  exist (`Ctx::finish`). A document with only one page built would show unresolved instances.
- `Document` and the engine assume every page of `doc.pages` is built; commands, undo, search and
  the layers panel iterate all of them.

## Plan if it is wanted

1. Keep decode as is (cannot start before the whole message is read). Peak memory needs the decoded
   tree anyway.
2. Split `to_rest` into `map_shared()` (styles, variables, library contexts, required mains: already
   computed before the pages in `Mapper::document`) and `map_page(i)`. Map the active page first,
   then the others in file order on a background thread, with the mapper moved into that thread.
3. Build the `Document` in two steps: shell (pages, shared resources, active page layers), then
   `Document::fill_page(page, nodes)` per remaining page. The engine needs a `PageState::Loading`
   flag: show a placeholder, block edits and commands touching that page, and run instance
   resolution again for the late page (instances of its mains on the active page start as frames).
4. Images and vector networks: images are already read by hash on demand per fill; networks could
   be decoded when a page is first built instead of up front.
5. Cost: the `Document`/engine changes (a loading page state, deferred instance and prototype
   resolution, UI placeholder) are the bulk, not the importer. Estimate several days plus tests.

## Parallelism that was evaluated

Per-page mapping in parallel changes the shared indexes and the layer cap, so the result differs.
Decode is one sequential stream. What is cheap and identical is overlapping teardown with work
(done: the REST JSON is freed on a background thread).
