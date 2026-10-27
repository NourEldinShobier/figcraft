# Lazy `.fig` import (design, not built)

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
