# Lazy `.fig` import

## Built: first page early (page-first open)

`fig::import_staged` (format) maps pages in file order with the same mapper as `import`. When the
first visible page is mapped (and more pages follow) it builds a document from that page alone
(library copies, styles and variables met so far; instances of mains on later pages stay
unlinked) and hands it to the caller. The import then continues unchanged, so the final document
is byte-identical to `import` (SHA-256 of latch.fig `b8ba057f...`, files-modern `35a71aaa...`
checked through the staged path with `FIGCRAFT_FIG_STAGED=1`).

The engine (`preload_staged`, `Session::poll_loading`) opens the early document as a tab with
`DocState.loading` set: edits, save and export are refused (`LOADING_READ_ONLY`), the import report
and font check wait. When the whole file arrives the tab keeps its uid, view and page and gets
the whole document (selection restarts). The app uses it for `.fig` files with at least
`STAGED_MIN_NODES` (100 000) node changes and several pages; a "Loading the other pages" banner
shows meanwhile. Wasm has no worker thread, so it keeps the eager path.

Measured on files-modern (542k layers, 4 pages, page 1 = 275k layers), loaded machine, release:
first page shown 48.6 s vs 97.0 s before; whole file 128 s vs 99.5 s (page 1 is built twice);
peak working set 2.9 GB vs 2.3 GB (the early document stays alive until the swap).

Not done (the rest of the plan below): the early stage still decodes the whole message and maps the
first page, so a file whose first page is most of the file gains little; the whole-file build
repeats the first page; memory is not reduced; the swap rebuilds the tab state on the UI thread
(about 2 s here); the first page is read-only instead of editable.

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
