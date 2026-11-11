//! `instances.fill` (a data merge: instances of a component made or updated from JSON or CSV rows)
//! and `component.swapAll` (every instance of one component swapped to another, keeping what
//! matches by name and reporting what does not).
//!
//! Both edit the document directly and are one undo step. A row or an instance that fails is put
//! back as it was (the document before it is restored) and reported, so one bad row does not hold
//! back the other 499.

use std::collections::{BTreeMap, BTreeSet};

use figcraft_doc::{Document, NodeId, NodeKind, PropDef};
use serde_json::{Map, Value, json};

use super::CommandSpec;
use super::component::{main_of, swap_instance};
use crate::components::{default_props, inside_instance, instances_of, prop_list, sync_instance, variant_options, variant_set};
use crate::params::{bool_of, f64_of, get, id_of, opt_id, req_id, string};
use crate::{EngineError, Result, Session, bad, spec};

const FILL: &str = "instances.fill";
const SWAP: &str = "component.swapAll";
/// Most rows one fill takes (split larger data over several calls, each its own undo step).
const MAX_ROWS: usize = 2000;
const MAX_CSV_BYTES: usize = 8_000_000;
const MAX_COLUMNS: usize = 200;
/// Most failures, unmatched rows and layer ids a result lists (the counts are always whole).
const LISTED: usize = 100;
const IDS_LISTED: usize = 500;
/// Most instances one swap takes.
const MAX_SWAPPED: usize = 20_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "instances.fill",
            "Fill instances from data",
            "{component? (a component or set: the one to make instances of; in update mode the one whose instances are updated), ids? (update mode: the instances to update), rows? ([{<column>: value}]) | csv? (text; the first line names the columns, quotes and commas as in RFC 4180, up to 2000 rows), map? ({<column>: <component property or layer name> | {layer, field? (text|visible)} | null (skip the column)}; a column without an entry is read by its own name: a text, boolean, instance swap or variant property of the component, else a layer in it: its text, or for a layer that is no text its visibility), mode? (create|update; default create), keyColumn? (update: the column whose value is the name of the instance to update; create: its value names the new instance), matchBy? (name|key; default name: the instance's layer name, or its layer key), parent? (create: a frame to put them in; default the page), x?, y? (create: where the first goes), gap? (between instances; default 16), direction? (vertical|horizontal; default vertical), continueOnError? (default true: a failing row is put back and reported, the others still apply; false: the first failure undoes everything)}  one undo step; a boolean or variant cell may be a string (\"true\", \"Large\"), an instance swap cell the id or the name of a component, an empty cell leaves anything but text alone, a null leaves a value alone → {mode, rows, created | updated, ids, failed: [{row, error}], unmatchedRows? ([{row, key}]: update rows no instance has the name of), unmatchedColumns? (columns nothing was read from), truncated?}; rows count from 0 after the header",
            false,
            fill
        ),
        spec!(
            "component.swapAll",
            "Swap a component everywhere",
            "{from (a component or set: its instances are swapped; a set takes the instances of every variant), to (a component or set: its default variant), ids? (only the instances that are, or are inside, these layers; default the whole file), mapOverrides? ({old name: new name}: layers and properties of the old component carry their overrides and values to the new component's one of the new name, for names that differ)}  keeps size, property values and overrides where layer or property names match (as instance.swap) and one undo step; instances inside other instances are left alone → {from, to, swapped, ids, skippedNested?, failed: [{id, error}], unmatched: {layers: [{name, count, id}] (overridden layers the new component has no layer for), props: [{name, count, id}] (set property values it has no property for)}, truncated?}. Use dryRun to see which instances change",
            true,
            swap_all
        ),
    ]
}

// ---------- rows ----------

fn rows_of(p: &Value) -> Result<Vec<Map<String, Value>>> {
    match (get(p, "rows"), get(p, "csv")) {
        (Some(_), Some(_)) => Err(bad(FILL, "give `rows` or `csv`, not both")),
        (Some(Value::Array(a)), None) => {
            if a.len() > MAX_ROWS {
                return Err(bad(FILL, format!("{} rows is more than the {MAX_ROWS} one call takes: split them over several calls", a.len())));
            }
            a.iter().enumerate().map(|(i, r)| r.as_object().cloned().ok_or_else(|| bad(FILL, format!("`rows[{i}]` must be an object")))).collect()
        }
        (Some(_), None) => Err(bad(FILL, "`rows` must be an array of objects")),
        (None, Some(Value::String(t))) => parse_csv(t),
        (None, Some(_)) => Err(bad(FILL, "`csv` must be text")),
        (None, None) => Err(bad(FILL, "give `rows` (an array of objects) or `csv` (text whose first line names the columns)")),
    }
}

/// CSV text as rows keyed by the header line's names. Comma separated, or tab separated when the
/// header has tabs and no commas; a quoted cell may hold the separator, line breaks and `""`.
fn parse_csv(text: &str) -> Result<Vec<Map<String, Value>>> {
    if text.len() > MAX_CSV_BYTES {
        return Err(bad(FILL, format!("`csv` is longer than {MAX_CSV_BYTES} bytes")));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let header_line = text.lines().next().unwrap_or_default();
    let sep = if header_line.contains('\t') && !header_line.contains(',') { '\t' } else { ',' };
    let (mut records, mut record, mut field) = (Vec::<Vec<String>>::new(), Vec::<String>::new(), String::new());
    let (mut quoted, mut was_quoted) = (false, false);
    let mut chars = text.chars().peekable();
    let end_record = |records: &mut Vec<Vec<String>>, record: &mut Vec<String>, field: &mut String, was_quoted: &mut bool| -> Result<()> {
        // A line with nothing on it is no row.
        if record.is_empty() && field.is_empty() && !*was_quoted {
            return Ok(());
        }
        record.push(std::mem::take(field));
        *was_quoted = false;
        records.push(std::mem::take(record));
        if records.len() > MAX_ROWS + 1 {
            return Err(bad(FILL, format!("`csv` has more than {MAX_ROWS} rows: split it over several calls")));
        }
        Ok(())
    };
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                c => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => (quoted, was_quoted) = (true, true),
            c if c == sep => {
                record.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_record(&mut records, &mut record, &mut field, &mut was_quoted)?;
            }
            c => field.push(c),
        }
    }
    if quoted {
        return Err(bad(FILL, "`csv` has a quoted cell that is never closed"));
    }
    end_record(&mut records, &mut record, &mut field, &mut was_quoted)?;
    let mut records = records.into_iter();
    let header: Vec<String> = records.next().unwrap_or_default().into_iter().map(|h| h.trim().to_string()).collect();
    if header.is_empty() || header.len() > MAX_COLUMNS {
        return Err(bad(FILL, format!("`csv` needs a header line of 1 to {MAX_COLUMNS} column names")));
    }
    if let Some(i) = header.iter().position(String::is_empty) {
        return Err(bad(FILL, format!("`csv` header column {} has no name", i + 1)));
    }
    let unique: BTreeSet<&String> = header.iter().collect();
    if unique.len() != header.len() {
        return Err(bad(FILL, "`csv` header names two columns alike"));
    }
    let mut rows = Vec::new();
    for (n, rec) in records.enumerate() {
        if rec.len() > header.len() {
            return Err(bad(FILL, format!("`csv` row {n} has {} cells but the header names {} columns", rec.len(), header.len())));
        }
        // A short row leaves its last columns out: they stay as they are.
        rows.push(header.iter().cloned().zip(rec).map(|(h, v)| (h, Value::String(v))).collect());
    }
    Ok(rows)
}

// ---------- columns ----------

/// A layer field a column writes.
#[derive(Clone, Copy)]
enum Field {
    Text,
    Visible,
}

/// What a `map` entry (or a column's own name) asks for.
#[derive(Clone)]
enum Wanted {
    /// A property of the component, else a layer of that name.
    Any(String),
    Layer(String, Field),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Boolean,
    Swap,
    Variant,
}

/// What a column resolves to for one component.
#[derive(Clone)]
enum Target {
    Prop(String, Kind),
    Layer(String, Field),
}

type ColumnMap = BTreeMap<String, Option<Wanted>>;

fn parse_map(p: &Value) -> Result<ColumnMap> {
    let Some(v) = get(p, "map") else { return Ok(ColumnMap::new()) };
    let Some(m) = v.as_object() else { return Err(bad(FILL, "`map` must be an object of column → property or layer")) };
    let mut out = ColumnMap::new();
    for (col, v) in m {
        let w = match v {
            Value::Null => None,
            Value::String(t) if !t.is_empty() => Some(Wanted::Any(t.clone())),
            Value::Object(o) => {
                let layer = o.get("layer").and_then(Value::as_str).filter(|l| !l.is_empty());
                let field = match o.get("field").and_then(Value::as_str) {
                    None | Some("text") => Field::Text,
                    Some("visible") => Field::Visible,
                    Some(_) => return Err(bad(FILL, format!("`map.{col}.field` must be text or visible"))),
                };
                Some(Wanted::Layer(layer.ok_or_else(|| bad(FILL, format!("`map.{col}` needs a `layer` name")))?.to_string(), field))
            }
            _ => return Err(bad(FILL, format!("`map.{col}` must be a property or layer name, {{layer, field?}} or null"))),
        };
        out.insert(col.clone(), w);
    }
    Ok(out)
}

/// What column `wanted` means for `comp`: one of its properties, else a layer inside it.
fn resolve(doc: &Document, comp: NodeId, wanted: &Wanted) -> Option<Target> {
    let name = match wanted {
        Wanted::Layer(n, f) => return Some(Target::Layer(n.clone(), *f)),
        Wanted::Any(n) => n,
    };
    if variant_set(doc, comp).is_some_and(|set| variant_options(doc, set).iter().any(|(o, _)| o == name)) {
        return Some(Target::Prop(name.clone(), Kind::Variant));
    }
    let def = prop_list(doc, comp).into_iter().find(|(k, _)| k == name).map(|(_, d)| d);
    match def {
        Some(PropDef::Text { .. }) => return Some(Target::Prop(name.clone(), Kind::Text)),
        Some(PropDef::Boolean { .. }) => return Some(Target::Prop(name.clone(), Kind::Boolean)),
        Some(PropDef::InstanceSwap { .. }) => return Some(Target::Prop(name.clone(), Kind::Swap)),
        Some(PropDef::Variant { .. }) => return Some(Target::Prop(name.clone(), Kind::Variant)),
        // A slot holds content, not a value: look for a layer of that name instead.
        Some(PropDef::Slot { .. }) | None => {}
    }
    let layer = doc.subtree(comp).into_iter().skip(1).filter_map(|i| doc.node(i)).find(|n| n.name == *name)?;
    Some(Target::Layer(name.clone(), if matches!(layer.kind, NodeKind::Text(_)) { Field::Text } else { Field::Visible }))
}

/// A cell as text: strings as they are, numbers and booleans as written.
fn cell_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// One fill: the column map, the key column and what each column resolved to so far.
struct Plan<'a> {
    map: &'a ColumnMap,
    cache: BTreeMap<(NodeId, String), Option<Target>>,
    /// Columns something was read from.
    used: BTreeSet<String>,
    /// Components and sets by name, built when a column names one.
    components: Option<BTreeMap<String, NodeId>>,
}

impl Plan<'_> {
    fn target(&mut self, doc: &Document, comp: NodeId, column: &str) -> Option<Target> {
        let wanted = match self.map.get(column) {
            Some(None) => return None,
            Some(Some(w)) => w.clone(),
            None => Wanted::Any(column.to_string()),
        };
        let key = (comp, column.to_string());
        self.cache.entry(key).or_insert_with(|| resolve(doc, comp, &wanted)).clone()
    }

    /// The component or set a cell names: its id, or its name.
    fn component(&mut self, doc: &Document, v: &Value) -> Option<NodeId> {
        if let Some(c) = id_of(v).and_then(|i| main_of(doc, i)) {
            return Some(c);
        }
        let name = v.as_str()?;
        let all = self.components.get_or_insert_with(|| {
            let mut m = BTreeMap::new();
            for n in doc.nodes().filter(|n| matches!(n.kind, NodeKind::Component { .. } | NodeKind::ComponentSet { .. })) {
                m.entry(n.name.clone()).or_insert(n.id);
            }
            m
        });
        all.get(name).and_then(|i| main_of(doc, *i))
    }
}

/// Write one row's cells into instance `inst`: the properties in one `instance.setProperties`, then
/// each layer's text or visibility.
fn apply_row(s: &mut Session, inst: NodeId, row: &Map<String, Value>, plan: &mut Plan) -> Result<()> {
    let mut props = Map::new();
    let mut layers: Vec<(String, Field, Value)> = Vec::new();
    {
        let doc = s.doc()?;
        let Some(NodeKind::Instance { component, .. }) = doc.node(inst).map(|n| &n.kind) else {
            return Err(bad(FILL, format!("{inst} is not an instance")));
        };
        let comp = *component;
        for (col, val) in row {
            if val.is_null() {
                continue;
            }
            let Some(target) = plan.target(doc, comp, col) else { continue };
            plan.used.insert(col.clone());
            let text = cell_text(val).ok_or_else(|| bad(FILL, format!("column `{col}`: a cell is text, a number or a boolean")))?;
            match target {
                Target::Prop(name, Kind::Text) => {
                    props.insert(name, json!(text));
                }
                // Anything but text: an empty cell means nothing.
                Target::Prop(..) | Target::Layer(_, Field::Visible) if text.trim().is_empty() => {}
                Target::Prop(name, Kind::Boolean) => {
                    let b = bool_of(val).ok_or_else(|| bad(FILL, format!("column `{col}`: \"{text}\" is not true or false")))?;
                    props.insert(name, json!(b));
                }
                Target::Prop(name, Kind::Variant) => {
                    props.insert(name, json!(text.trim()));
                }
                Target::Prop(name, Kind::Swap) => {
                    let c =
                        plan.component(doc, val).ok_or_else(|| bad(FILL, format!("column `{col}`: no component with the id or name \"{text}\"")))?;
                    props.insert(name, json!(c));
                }
                Target::Layer(name, Field::Text) => layers.push((name, Field::Text, json!(text))),
                Target::Layer(name, Field::Visible) => {
                    let b = bool_of(val).ok_or_else(|| bad(FILL, format!("column `{col}`: \"{text}\" is not true or false")))?;
                    layers.push((name, Field::Visible, json!(b)));
                }
            }
        }
    }
    if !props.is_empty() {
        run(s, "instance.setProperties", json!({"ids": [inst], "properties": props}))?;
    }
    for (name, field, value) in layers {
        let layer = {
            let doc = s.doc()?;
            doc.subtree(inst).into_iter().skip(1).find(|i| doc.node(*i).is_some_and(|n| n.name == name))
        };
        let layer = layer.ok_or_else(|| bad(FILL, format!("the instance has no layer named \"{name}\"")))?;
        match field {
            Field::Text => run(s, "text.set", json!({"ids": [layer], "characters": value}))?,
            Field::Visible => run(s, "node.set", json!({"ids": [layer], "props": {"visible": value}}))?,
        };
    }
    Ok(())
}

/// Run command `id` as a step of this command's group.
fn run(s: &mut Session, id: &str, params: Value) -> Result<Value> {
    s.execute(id, &params)
}

// ---------- instances.fill ----------

/// Cap a list for a result, saying so.
fn listed<T>(mut v: Vec<T>, truncated: &mut bool, cap: usize) -> Vec<T> {
    if v.len() > cap {
        v.truncate(cap);
        *truncated = true;
    }
    v
}

/// The whole fill is one undo step: its edits run as the commands a person would (so instances
/// rebuild after each), inside one group.
fn fill(s: &mut Session, p: &Value) -> Result<Value> {
    s.grouped("Fill instances from data", |s| fill_rows(s, p))
}

fn fill_rows(s: &mut Session, p: &Value) -> Result<Value> {
    let rows = rows_of(p)?;
    if rows.is_empty() {
        return Err(bad(FILL, "there are no rows"));
    }
    let update = match string(p, "mode").as_deref() {
        None | Some("create") => false,
        Some("update") => true,
        Some(_) => return Err(bad(FILL, "`mode` must be create or update")),
    };
    let keep_going = match get(p, "continueOnError") {
        None => true,
        Some(v) => bool_of(v).ok_or_else(|| bad(FILL, "`continueOnError` must be a boolean (true or false)"))?,
    };
    let key_column = match get(p, "keyColumn") {
        None => None,
        Some(Value::String(c)) if !c.is_empty() => Some(c.clone()),
        Some(_) => return Err(bad(FILL, "`keyColumn` must be a column name")),
    };
    let by_key = match string(p, "matchBy").as_deref() {
        None | Some("name") => false,
        Some("key") => true,
        Some(_) => return Err(bad(FILL, "`matchBy` must be name or key")),
    };
    let map = parse_map(p)?;
    let all_columns: BTreeSet<String> = rows.iter().flat_map(|r| r.keys().cloned()).collect();
    if all_columns.len() > MAX_COLUMNS {
        return Err(bad(FILL, format!("the rows have more than {MAX_COLUMNS} different columns")));
    }
    let mut plan = Plan { map: &map, cache: BTreeMap::new(), used: BTreeSet::new(), components: None };
    let mut failed: Vec<Value> = Vec::new();
    let mut failures = 0usize;
    let mut done: Vec<NodeId> = Vec::new();
    let mut unmatched_rows: Vec<Value> = Vec::new();
    let mut unmatched = 0usize;

    let (comp, instances) = if update {
        if key_column.is_none() {
            return Err(bad(FILL, "update mode needs `keyColumn`: the column that names the instance each row updates"));
        }
        update_candidates(s, p)?
    } else {
        (Some(create_target(s, p)?), Vec::new())
    };

    let (parent, mut at, gap, down) = if update { (s.page()?, (0.0, 0.0), 0.0, true) } else { placement(s, p, comp)? };
    // Instances by the name or key a row looks them up by.
    let mut index: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
    if update {
        let doc = s.doc()?;
        for i in &instances {
            let Some(n) = doc.node(*i) else { continue };
            let k = if by_key { n.key().map(str::to_string) } else { Some(n.name.clone()) };
            if let Some(k) = k {
                index.entry(k).or_default().push(*i);
            }
        }
    }

    for (n, row) in rows.iter().enumerate() {
        let key = key_column.as_ref().and_then(|c| row.get(c)).and_then(cell_text).map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        let outcome: Result<Vec<NodeId>> = if update {
            match key.as_ref().and_then(|k| index.get(k)) {
                None => {
                    unmatched += 1;
                    if unmatched_rows.len() < LISTED {
                        unmatched_rows.push(json!({"row": n, "key": key}));
                    }
                    continue;
                }
                // A failing row is put back as it was (the group goes on).
                Some(found) => s.checkpointed(None, |s| found.iter().try_for_each(|i| apply_row(s, *i, row, &mut plan)).map(|()| found.clone())),
            }
        } else {
            s.checkpointed(None, |s| create_one(s, comp, (parent, &mut at, gap, down), key.as_deref(), row, &mut plan).map(|i| vec![i]))
        };
        match outcome {
            Ok(ids) => done.extend(ids),
            Err(EngineError::Waiting) => return Err(EngineError::Waiting),
            Err(e) if keep_going => {
                failures += 1;
                if failed.len() < LISTED {
                    failed.push(json!({"row": n, "error": e.to_string()}));
                }
            }
            Err(e) => return Err(bad(FILL, format!("row {n}: {e} (nothing was changed)"))),
        }
    }

    let mut truncated = failures > failed.len() || unmatched > unmatched_rows.len();
    let count = done.len();
    let ids = listed(done.clone(), &mut truncated, IDS_LISTED);
    if !update {
        s.set_selection(done);
    }
    let ignored: BTreeSet<&String> = map.iter().filter(|(_, w)| w.is_none()).map(|(c, _)| c).collect();
    let unmatched_columns: Vec<&String> =
        all_columns.iter().filter(|c| !plan.used.contains(*c) && !ignored.contains(c) && Some(*c) != key_column.as_ref()).collect();
    let mut out = json!({
        "mode": if update { "update" } else { "create" },
        "rows": rows.len(),
        "ids": ids,
        "failed": failed,
    });
    out[if update { "updated" } else { "created" }] = json!(count);
    if failures > 0 {
        out["failedCount"] = json!(failures);
    }
    if unmatched > 0 {
        out["unmatchedRows"] = json!(unmatched_rows);
        out["unmatchedRowCount"] = json!(unmatched);
    }
    if !unmatched_columns.is_empty() {
        out["unmatchedColumns"] = json!(unmatched_columns);
    }
    if truncated {
        out["truncated"] = json!(true);
    }
    Ok(out)
}

/// The main component new instances are made of.
fn create_target(s: &Session, p: &Value) -> Result<NodeId> {
    let c = req_id(FILL, p, "component")?;
    main_of(s.doc()?, c).ok_or_else(|| bad(FILL, "`component` must be a component or component set"))
}

/// Update mode: the instances rows can name (those of `component`, or `ids`) and, for the first
/// kind, the component.
fn update_candidates(s: &Session, p: &Value) -> Result<(Option<NodeId>, Vec<NodeId>)> {
    let doc = s.doc()?;
    if let Some(v) = get(p, "ids") {
        let Value::Array(a) = v else { return Err(bad(FILL, "`ids` must be an array of instance ids")) };
        let mut out = Vec::new();
        for v in a {
            let i = id_of(v).ok_or_else(|| bad(FILL, format!("`ids`: {v} is not a layer id")))?;
            if !doc.node(i).is_some_and(|n| matches!(n.kind, NodeKind::Instance { .. })) {
                return Err(bad(FILL, format!("{i} is not an instance")));
            }
            out.push(i);
        }
        return Ok((None, out));
    }
    let c = get(p, "component").ok_or_else(|| bad(FILL, "update mode needs `component` or `ids`: which instances to update"))?;
    let c = id_of(c).ok_or_else(|| bad(FILL, "`component` must be a node id"))?;
    let comps: Vec<NodeId> = match doc.node(c).map(|n| &n.kind) {
        Some(NodeKind::ComponentSet { .. }) => doc
            .node(c)
            .map(|n| n.children.iter().copied().filter(|k| doc.node(*k).is_some_and(|x| matches!(x.kind, NodeKind::Component { .. }))).collect())
            .unwrap_or_default(),
        Some(NodeKind::Component { .. }) => vec![c],
        _ => return Err(bad(FILL, "`component` must be a component or component set")),
    };
    Ok((Some(c), comps.into_iter().flat_map(|k| instances_of(doc, k)).collect()))
}

/// Where new instances go: the parent, the first position (parent space), the gap and whether they
/// stack downwards.
fn placement(s: &Session, p: &Value, comp: Option<NodeId>) -> Result<(NodeId, (f64, f64), f64, bool)> {
    let parent = match opt_id(FILL, p, "parent")? {
        Some(pa) => pa,
        None => s.page()?,
    };
    let doc = s.doc()?;
    if !doc.contains(parent) {
        return Err(EngineError::NoNode(parent));
    }
    let start = comp
        .and_then(|c| doc.world_bounds(c))
        .map(|b| (b.x1 + 40.0, b.y0))
        .filter(|_| doc.node(parent).is_some_and(|n| n.is_page()))
        .unwrap_or((0.0, 0.0));
    let coord = |key: &str, d: f64| match get(p, key) {
        None => Ok(d),
        Some(v) => f64_of(v).ok_or_else(|| bad(FILL, format!("`{key}` must be a finite number"))),
    };
    let gap = coord("gap", 16.0)?;
    if !(0.0..=100_000.0).contains(&gap) {
        return Err(bad(FILL, "`gap` must be from 0 to 100000"));
    }
    let down = match string(p, "direction").as_deref() {
        None | Some("vertical") => true,
        Some("horizontal") => false,
        Some(_) => return Err(bad(FILL, "`direction` must be vertical or horizontal")),
    };
    Ok((parent, (coord("x", start.0)?, coord("y", start.1)?), gap, down))
}

/// Make one instance for `row` at `at` and move `at` past it.
fn create_one(
    s: &mut Session,
    comp: Option<NodeId>,
    (parent, at, gap, down): (NodeId, &mut (f64, f64), f64, bool),
    name: Option<&str>,
    row: &Map<String, Value>,
    plan: &mut Plan,
) -> Result<NodeId> {
    let comp = comp.ok_or_else(|| bad(FILL, "no component"))?;
    let made = run(s, "instance.create", json!({"component": comp, "x": at.0, "y": at.1, "parent": parent}))?;
    let inst = made.get("id").and_then(id_of).ok_or_else(|| bad(FILL, "the instance was not made"))?;
    if let Some(name) = name {
        let name: String = name.chars().take(255).collect();
        run(s, "node.set", json!({"ids": [inst], "props": {"name": name}}))?;
    }
    apply_row(s, inst, row, plan)?;
    if let Some(n) = s.doc()?.node(inst) {
        if down {
            at.1 += n.height + gap;
        } else {
            at.0 += n.width + gap;
        }
    }
    Ok(inst)
}

// ---------- component.swapAll ----------

/// What swapping one instance dropped.
#[derive(Default)]
struct Dropped {
    layers: Vec<String>,
    props: Vec<String>,
}

fn overridden_layers(doc: &Document, inst: NodeId) -> Vec<String> {
    doc.subtree(inst).into_iter().skip(1).filter_map(|i| doc.node(i)).filter(|n| !n.overrides.is_empty()).map(|n| n.name.clone()).collect()
}

/// Swap instance `i` to `to`, first renaming layers and properties as `renames` says so they match
/// the new component's. What the new component has no counterpart for comes back.
fn swap_one(doc: &mut Document, i: NodeId, to: NodeId, renames: &BTreeMap<String, String>, report: bool) -> std::result::Result<Dropped, String> {
    let ren = |n: &str| renames.get(n).cloned().unwrap_or_else(|| n.to_string());
    let old_defaults = match doc.node(i).map(|n| &n.kind) {
        Some(NodeKind::Instance { component, .. }) => doc.node(*component).map(default_props).unwrap_or_default(),
        _ => return Err(format!("{i} is not an instance")),
    };
    let before: Vec<String> = overridden_layers(doc, i).iter().map(|n| ren(n)).collect();
    // The property values this instance set itself (not the old component's defaults), by their new names.
    let set_props: Vec<String> = match doc.node(i).map(|n| &n.kind) {
        Some(NodeKind::Instance { props, .. }) => props.iter().filter(|(k, v)| old_defaults.get(*k) != Some(*v)).map(|(k, _)| ren(k)).collect(),
        _ => Vec::new(),
    };
    if !renames.is_empty() {
        for l in doc.subtree(i).into_iter().skip(1) {
            let new = doc.node(l).and_then(|n| renames.get(&n.name)).cloned();
            if let (Some(new), Ok(n)) = (new, doc.get_mut(l)) {
                n.name = new;
            }
        }
        if let Ok(n) = doc.get_mut(i)
            && let NodeKind::Instance { props, .. } = &mut n.kind
        {
            for (old, new) in renames {
                if !props.contains_key(new)
                    && let Some(v) = props.remove(old)
                {
                    props.insert(new.clone(), v);
                }
            }
        }
    }
    swap_instance(doc, i, to)?;
    // The rebuild that shows what carried over runs on a scratch copy: in the file it happens after
    // the command, like any swap, because the structure check refuses a rebuilt instance.
    if !report {
        return Ok(Dropped::default());
    }
    sync_instance(doc, i);
    let after: BTreeSet<String> = overridden_layers(doc, i).into_iter().collect();
    let new_defaults = doc.node(to).map(default_props).unwrap_or_default();
    Ok(Dropped {
        layers: before.into_iter().filter(|n| !after.contains(n)).collect(),
        props: set_props.into_iter().filter(|k| !new_defaults.contains_key(k)).collect(),
    })
}

fn swap_all(s: &mut Session, p: &Value) -> Result<Value> {
    let from = req_id(SWAP, p, "from")?;
    let to = req_id(SWAP, p, "to")?;
    let renames = match get(p, "mapOverrides") {
        None => BTreeMap::new(),
        Some(Value::Object(m)) if m.len() <= 200 => {
            let mut out = BTreeMap::new();
            for (old, new) in m {
                match new.as_str().filter(|n| !n.is_empty()) {
                    Some(n) => out.insert(old.clone(), n.to_string()),
                    None => return Err(bad(SWAP, format!("`mapOverrides.{old}` must be the new name"))),
                };
            }
            out
        }
        Some(_) => return Err(bad(SWAP, "`mapOverrides` must be an object of old name → new name (up to 200)")),
    };
    let roots: Option<Vec<NodeId>> = match get(p, "ids") {
        None => None,
        Some(Value::Array(a)) => {
            Some(a.iter().map(|v| id_of(v).ok_or_else(|| bad(SWAP, format!("`ids`: {v} is not a layer id")))).collect::<Result<_>>()?)
        }
        Some(_) => return Err(bad(SWAP, "`ids` must be an array of layer ids")),
    };
    let doc = s.doc()?;
    let comps: Vec<NodeId> = match doc.node(from).map(|n| &n.kind) {
        Some(NodeKind::ComponentSet { .. }) => doc
            .node(from)
            .map(|n| n.children.iter().copied().filter(|k| doc.node(*k).is_some_and(|x| matches!(x.kind, NodeKind::Component { .. }))).collect())
            .unwrap_or_default(),
        Some(NodeKind::Component { .. }) => vec![from],
        _ => return Err(bad(SWAP, "`from` must be a component or component set")),
    };
    let target = main_of(doc, to).ok_or_else(|| bad(SWAP, "`to` must be a component or component set"))?;
    if comps == [target] {
        return Err(bad(SWAP, "`from` and `to` are the same component"));
    }
    if let Some(r) = roots.iter().flatten().find(|r| !doc.contains(**r)) {
        return Err(EngineError::NoNode(*r));
    }
    let mut found: Vec<NodeId> = Vec::new();
    let mut nested = 0usize;
    for c in &comps {
        for i in instances_of(doc, *c) {
            if roots.as_ref().is_some_and(|rs| !rs.iter().any(|r| doc.is_ancestor(*r, i))) {
                continue;
            }
            if inside_instance(doc, i) {
                nested += 1;
            } else {
                found.push(i);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    if found.len() > MAX_SWAPPED {
        return Err(bad(SWAP, format!("{} instances is more than the {MAX_SWAPPED} one call swaps: narrow it with `ids`", found.len())));
    }

    let mut swapped: Vec<NodeId> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut failures = 0usize;
    let mut layers: BTreeMap<String, (usize, NodeId)> = BTreeMap::new();
    let mut props: BTreeMap<String, (usize, NodeId)> = BTreeMap::new();
    // What carries over is read from a scratch copy that is rebuilt after each swap.
    let mut scratch = (*s.ds()?.doc).clone();
    for i in found {
        let before = s.ds()?.doc.clone();
        let outcome = swap_one(&mut scratch, i, target, &renames, true).and_then(|d| {
            let doc = s.doc_mut().map_err(|e| e.to_string())?;
            swap_one(doc, i, target, &renames, false).map(|_| d)
        });
        match outcome {
            Ok(d) => {
                swapped.push(i);
                for (list, names) in [(&mut layers, d.layers), (&mut props, d.props)] {
                    for n in names {
                        list.entry(n).or_insert((0, i)).0 += 1;
                    }
                }
            }
            Err(e) => {
                s.ds_mut()?.doc = before;
                failures += 1;
                if failed.len() < LISTED {
                    failed.push(json!({"id": i, "error": e}));
                }
            }
        }
    }
    let mut truncated = failures > failed.len();
    let count = swapped.len();
    let ids = listed(swapped, &mut truncated, IDS_LISTED);
    let named = |m: BTreeMap<String, (usize, NodeId)>, truncated: &mut bool| {
        let all: Vec<Value> = m.into_iter().map(|(name, (count, id))| json!({"name": name, "count": count, "id": id})).collect();
        listed(all, truncated, LISTED)
    };
    let mut out = json!({
        "from": from,
        "to": target,
        "swapped": count,
        "ids": ids,
        "failed": failed,
        "unmatched": {"layers": named(layers, &mut truncated), "props": named(props, &mut truncated)},
    });
    if nested > 0 {
        out["skippedNested"] = json!(nested);
    }
    if truncated {
        out["truncated"] = json!(true);
    }
    Ok(out)
}
