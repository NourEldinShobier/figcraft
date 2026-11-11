use serde_json::{Value, json};

use crate::Session;

const PAGE: &str = r##"<!doctype html><html><head><title>Acme</title>
<style>
  :root { --brand: #2563eb; --pad: 16px; }
  body { margin: 0; font-family: "Inter", sans-serif; color: #111827; }
  .nav { display: flex; justify-content: space-between; align-items: center; padding: 16px 32px; background: #ffffff; border-bottom: 1px solid #e5e7eb; }
  .nav a { color: var(--brand); text-decoration: none; margin-left: 16px; }
  .logo { font-weight: 700; font-size: 20px; }
  .hero { padding: 64px 32px; text-align: center; background: linear-gradient(to bottom, #eff6ff, #ffffff); }
  .hero h1 { font-size: 48px; margin: 0 0 16px; }
  .hero p { font-size: 18px; color: #4b5563; margin: 0 0 24px; }
  .btn { background: var(--brand); color: #fff; padding: 12px 24px; border-radius: 8px; border: none; }
  .grid { display: flex; gap: 24px; padding: 32px; }
  .card { flex: 1; padding: var(--pad); border: 1px solid #e5e7eb; border-radius: 12px; box-shadow: 0 1px 3px rgba(0,0,0,.1); }
  .card h3 { margin: 0 0 8px; }
</style></head>
<body>
  <div class="nav"><span class="logo">Acme</span><div><a href="#">Docs</a><a href="#">Pricing</a></div></div>
  <section class="hero"><h1>Build <b>faster</b></h1><p>Ship designs and code together.</p><button class="btn">Get started</button></section>
  <div class="grid">
    <div class="card"><h3>One</h3><p>First card.</p></div>
    <div class="card"><h3>Two</h3><p>Second card.</p></div>
    <div class="card"><h3>Three</h3><p>Third card.</p></div>
  </div>
</body></html>"##;

fn ex(s: &mut Session, html: &str) -> Value {
    s.execute("import.html", &json!({"html": html})).unwrap_or_else(|e| panic!("import.html: {e}"))
}

/// Layers named exactly `name` (node.find matches substrings).
fn ids(s: &mut Session, name: &str) -> Vec<u64> {
    let r = s.execute("node.find", &json!({"name": name})).unwrap();
    let found: Vec<u64> = r["ids"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    found.into_iter().filter(|id| s.execute("node.get", &json!({"id": id})).unwrap()["name"] == json!(name)).collect()
}

fn get(s: &mut Session, name: &str) -> Value {
    let id = *ids(s, name).first().unwrap_or_else(|| panic!("no layer named {name}"));
    s.execute("node.get", &json!({"id": id})).unwrap()
}

fn warnings(r: &Value) -> Vec<String> {
    r["warnings"].as_array().unwrap().iter().filter_map(|w| w.as_str().map(str::to_string)).collect()
}

#[test]
fn a_small_page_becomes_nested_auto_layout_frames() {
    let mut s = Session::new();
    let r = ex(&mut s, PAGE);
    assert!(warnings(&r).is_empty(), "{r}");
    assert!(r["created"].as_u64().unwrap() >= 20, "{r}");

    let root = s.execute("node.get", &json!({"id": r["rootId"]})).unwrap();
    assert_eq!(
        (root["name"].clone(), root["width"].clone(), root["layout"]["mode"].clone()),
        (json!("Acme"), json!(1440.0), json!("vertical")),
        "{root}"
    );
    assert_eq!(root["sizing"]["vertical"], json!("hug"));

    let nav = get(&mut s, "div.nav");
    assert_eq!(nav["layout"]["mode"], json!("horizontal"), "{nav}");
    assert_eq!((nav["layout"]["mainAlign"].clone(), nav["layout"]["crossAlign"].clone()), (json!("spaceBetween"), json!("center")));
    assert_eq!(nav["layout"]["padding"], json!([16.0, 32.0, 16.0, 32.0]));
    assert_eq!((nav["strokeSides"].clone(), nav["strokes"][0]["color"].clone()), (json!([0.0, 0.0, 1.0, 0.0]), json!("#E5E7EB")));
    assert_eq!(nav["clip"], json!(false));

    let h1 = get(&mut s, "Build faster");
    assert_eq!((h1["fontSize"].clone(), h1["fontStyle"].clone(), h1["textAlign"].clone()), (json!(48.0), json!("Bold"), json!("center")), "{h1}");
    assert_eq!(h1["fills"][0]["color"], json!("#111827"));

    let hero = get(&mut s, "section.hero");
    assert_eq!((hero["fills"][0]["type"].clone(), hero["fills"][0]["kind"].clone()), (json!("gradient"), json!("linear")), "{hero}");
    assert_eq!(hero["layout"]["crossAlign"], json!("center"), "text-align: center centres hugging children");

    let btn = get(&mut s, "button.btn");
    assert_eq!(
        (btn["cornerRadius"].clone(), btn["fills"][0]["color"].clone(), btn["layout"]["padding"].clone()),
        (json!(8.0), json!("#2563EB"), json!([12.0, 24.0, 12.0, 24.0])),
        "custom properties resolve: {btn}"
    );

    let grid = get(&mut s, "div.grid");
    assert_eq!((grid["layout"]["mode"].clone(), grid["layout"]["gap"].clone()), (json!("horizontal"), json!(24.0)), "{grid}");
    let cards = ids(&mut s, "div.card");
    assert_eq!(cards.len(), 3);
    for id in cards {
        let c = s.execute("node.get", &json!({"id": id})).unwrap();
        assert_eq!(c["sizing"]["horizontal"], json!("fill"), "flex: 1 fills the row: {c}");
        assert_eq!(c["cornerRadius"], json!(12.0));
        assert_eq!(c["effects"][0]["type"], json!("dropShadow"));
        assert_eq!(c["effects"][0]["offset"], json!([0.0, 1.0]));
        assert!((c["width"].as_f64().unwrap() - 442.67).abs() < 0.01, "{c}");
        assert_eq!(c["height"], json!(115.0), "the card hugs its content: {c}");
    }
}

#[test]
fn the_viewport_width_and_a_whole_import_undo_as_one_step() {
    let mut s = Session::new();
    let before = s.undo_depth();
    let r = s.execute("import.html", &json!({"html": "<div style='width:50%'>half</div>", "width": 800, "x": 10, "y": 20})).unwrap();
    let root = s.execute("node.get", &json!({"id": r["rootId"]})).unwrap();
    assert_eq!((root["width"].clone(), root["x"].clone(), root["y"].clone()), (json!(800.0), json!(10.0), json!(20.0)), "{root}");
    let half = get(&mut s, "half");
    assert_eq!(half["width"], json!(400.0), "50% of the 800 px viewport: {half}");
    assert_eq!(s.undo_depth().0, before.0 + 1, "one undo step");
    assert!(s.undo().unwrap());
    assert!(ids(&mut s, "half").is_empty());
}

#[test]
fn dry_run_leaves_the_document_alone() {
    let mut s = Session::new();
    let r = s.execute("import.html", &json!({"html": "<p>x</p>", "dryRun": true})).unwrap();
    assert_eq!(r["dryRun"], json!(true), "{r}");
    assert!(ids(&mut s, "x").is_empty());
}

#[test]
fn unsupported_features_are_listed_and_the_rest_is_still_imported() {
    let mut s = Session::new();
    let html = r#"<style>
      @media print { p { color: red } }
      a:hover { color: blue }
      ul > li { margin: 0 }
      .g { display: flex; transform: rotate(3deg); }
      .w { width: calc(100% - 10px); position: absolute; }
    </style>
    <div class="g"><p class="w">kept</p></div>
    <video src="x.mp4"></video><input type="text"><img src="https://example.com/a.png" alt="Photo">
    <link rel="stylesheet" href="site.css">"#;
    let r = ex(&mut s, html);
    let w = warnings(&r).join("\n");
    for needle in ["media query", "a:hover", "ul > li", "transform", "calc(100% - 10px)", "position", "<video>", "<input>", "<img>", "stylesheet"] {
        assert!(w.contains(needle), "no warning mentions {needle}: {w}");
    }
    assert!(!get(&mut s, "kept")["characters"].is_null());
    let photo = get(&mut s, "Photo");
    assert_eq!(photo["type"], json!("RECTANGLE"), "a remote image is a placeholder: {photo}");
    assert_eq!((photo["width"].clone(), photo["height"].clone()), (json!(300.0), json!(150.0)));
    assert_eq!(get(&mut s, "video")["type"], json!("RECTANGLE"));
}

const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[test]
fn data_uri_images_and_inline_svg_become_layers_and_other_sources_are_never_fetched() {
    let mut s = Session::new();
    let html = format!(
        r##"<img alt="Pixel" width="40" height="20" src="data:image/png;base64,{PNG_1X1}">
        <svg width="24" height="24" viewBox="0 0 24 24"><rect width="24" height="24" fill="#ff0000"/></svg>
        <img alt="Bad" width="10" height="10" src="data:image/png;base64,AAAA">"##
    );
    let r = ex(&mut s, &html);
    let img = get(&mut s, "Pixel");
    assert_eq!((img["type"].clone(), img["width"].clone(), img["height"].clone()), (json!("RECTANGLE"), json!(40.0), json!(20.0)), "{img}");
    assert_eq!(img["fills"][0]["type"], json!("image"), "{img}");
    let svg = get(&mut s, "svg");
    assert_eq!((svg["width"].clone(), svg["height"].clone()), (json!(24.0), json!(24.0)), "{svg}");
    assert_eq!(svg["type"], json!("FRAME"), "{svg}");
    assert!(warnings(&r).iter().any(|w| w.contains("could not be read")), "{r}");
}

#[test]
fn margins_become_padding_frames_and_auto_margins_centre_a_block() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<div id=box style='width:300px;height:40px;margin:10px 0 20px;background:#00f'></div><div id=wide style='max-width:600px;margin:0 auto;padding:8px'>centred</div>",
    );
    let boxed = get(&mut s, "div#box margin");
    assert_eq!(
        (boxed["layout"]["padding"].clone(), boxed["height"].clone(), boxed["sizing"]["horizontal"].clone()),
        (json!([10.0, 0.0, 20.0, 0.0]), json!(70.0), json!("hug")),
        "{boxed}"
    );
    let wide = get(&mut s, "div#wide margin");
    assert_eq!(
        (wide["layout"]["mode"].clone(), wide["layout"]["mainAlign"].clone(), wide["sizing"]["horizontal"].clone()),
        (json!("horizontal"), json!("center"), json!("fill")),
        "{wide}"
    );
    let inner = get(&mut s, "div#wide");
    assert_eq!(inner["maxWidth"], json!(616.0), "max-width is the content box: 600 + 2 x 8 padding: {inner}");
}

#[test]
fn flex_properties_map_to_auto_layout() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<div id=a style='display:flex;flex-direction:column;gap:12px;align-items:center;justify-content:flex-end;height:200px'><span>one</span><span>two</span></div>
         <div id=b style='display:flex;flex-wrap:wrap;gap:8px 16px;flex-direction:row-reverse'><i>first</i><i>second</i></div>",
    );
    let a = get(&mut s, "div#a");
    assert_eq!(a["layout"]["mode"], json!("vertical"), "{a}");
    assert_eq!(
        (a["layout"]["gap"].clone(), a["layout"]["crossAlign"].clone(), a["layout"]["mainAlign"].clone()),
        (json!(12.0), json!("center"), json!("max"))
    );
    assert_eq!((a["height"].clone(), a["sizing"]["vertical"].clone()), (json!(200.0), json!("fixed")));
    let b = get(&mut s, "div#b");
    assert_eq!(
        (b["layout"]["wrap"].clone(), b["layout"]["gap"].clone(), b["layout"]["crossGap"].clone()),
        (json!(true), json!(16.0), json!(8.0)),
        "{b}"
    );
    let kids: Vec<String> = b["children"].as_array().unwrap().iter().map(|c| c["characters"].as_str().unwrap().to_string()).collect();
    assert_eq!(kids, ["second", "first"], "row-reverse reverses the items");
}

#[test]
fn the_cascade_orders_by_importance_specificity_and_source() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<style>p { color: #111111 } .c { color: #222222 } div p { color: #444444 } .i { color: #555555 !important }</style>
         <p>a</p><p class=c>b</p><p class=c style='color:#aaaaaa'>c</p><div><p>d</p></div><div><p class=c>e</p></div><p class='c i' style='color:#bbbbbb'>f</p>",
    );
    let colour = |s: &mut Session, text: &str| get(s, text)["fills"][0]["color"].clone();
    assert_eq!(colour(&mut s, "a"), json!("#111111"));
    assert_eq!(colour(&mut s, "b"), json!("#222222"), "a class beats a tag");
    assert_eq!(colour(&mut s, "c"), json!("#AAAAAA"), "the style attribute beats a class");
    assert_eq!(colour(&mut s, "d"), json!("#444444"), "`div p` (0,0,2) beats `p`");
    assert_eq!(colour(&mut s, "e"), json!("#222222"), ".c (0,1,0) beats `div p` (0,0,2)");
    assert_eq!(colour(&mut s, "f"), json!("#555555"), "!important beats the style attribute");
}

#[test]
fn inline_runs_with_boxes_become_wrapping_rows() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<p>Hello <b style='color:#ff0000'>big</b> world<br>next line</p><div><a style='padding:4px 8px;background:#eee'>One</a> <a style='padding:4px 8px;background:#eee'>Two</a></div>",
    );
    let p = get(&mut s, "Hello big world");
    assert_eq!(p["characters"], json!("Hello big world\nnext line"), "{p}");
    let row = get(&mut s, "line");
    assert_eq!(
        (row["layout"]["mode"].clone(), row["layout"]["wrap"].clone(), row["children"].as_array().map(Vec::len)),
        (json!("horizontal"), json!(true), Some(2)),
        "{row}"
    );
}

#[test]
fn page_size_and_layer_caps_hold() {
    let mut s = Session::new();
    let big = format!("<p>{}</p>", "x".repeat(super::MAX_INPUT));
    let e = s.execute("import.html", &json!({"html": big})).unwrap_err().to_string();
    assert!(e.contains("2 MB"), "{e}");

    let many = "<p>row</p>".repeat(super::MAX_LAYERS + 500);
    let r = ex(&mut s, &many);
    assert_eq!(r["truncated"], json!(true), "{r}");
    assert!(r["created"].as_u64().unwrap() <= super::MAX_LAYERS as u64, "{r}");
    assert!(warnings(&r).iter().any(|w| w.contains("5000")), "{r}");

    let deep = format!("{}x{}", "<div>".repeat(500), "</div>".repeat(500));
    let r = ex(&mut s, &deep);
    assert!(r["created"].as_u64().unwrap() > 50, "deep nesting imports down to the limit: {r}");
    assert!(warnings(&r).iter().any(|w| w.contains("deeper")), "{r}");
}

#[test]
fn junk_input_is_an_error_or_a_page_never_a_panic() {
    let mut s = Session::new();
    for junk in [
        json!({}),
        json!({"html": 5}),
        json!({"html": ""}),
        json!({"html": "   "}),
        json!({"html": "<p>x</p>", "width": 0}),
        json!({"html": "<p>x</p>", "width": "wide"}),
        json!({"html": "<p>x</p>", "width": 1e9}),
        json!({"html": "<p>x</p>", "parent": 99999}),
        json!({"html": "<p>x</p>", "parent": "nope"}),
        json!({"html": "<p>x</p>", "x": "left"}),
        json!({"html": "<p>x</p>", "y": null, "x": null}),
    ] {
        let _ = s.execute("import.html", &junk);
    }
    for html in [
        "<",
        "<<<>>>",
        "</div></p>",
        "<a href=\"x",
        "<style>",
        "<style>{",
        "<style>a{color:}</style>x",
        "<svg",
        "<svg><svg>",
        "<img src=data:>",
        "<img src=\"data:image/svg+xml;base64,@@@\">",
        "<div style='width:99999999999px;height:-5px;padding:-1px;margin:1e400px;opacity:NaN;border:1e99px solid red;box-shadow:1px;background:linear-gradient(;color:rgb(;font:;flex:;gap:'>t</div>",
        "<div style='display:flex;flex-wrap:wrap;gap:calc(1px);font-size:0;line-height:-1'>a<div>b</div>c</div>",
        "<p style='background:linear-gradient(red,)'>x</p><p style='background:linear-gradient(to nowhere, red, blue)'>y</p>",
        "<table><tr><td>1<td>2<tr><td>3</table>",
        "\u{0}\u{1f600}<b>\u{202e}x",
    ] {
        let _ = s.execute("import.html", &json!({"html": html}));
    }
}

#[test]
fn tables_and_lists_import_as_frames() {
    let mut s = Session::new();
    ex(&mut s, "<table><tr><td>1</td><td>2</td></tr><tr><td>3</td><td>4</td></tr></table><ul><li>a</li><li>b</li></ul>");
    let row = get(&mut s, "tr");
    assert_eq!(row["layout"]["mode"], json!("horizontal"), "{row}");
    assert_eq!(get(&mut s, "ul")["layout"]["padding"], json!([0.0, 0.0, 0.0, 40.0]));
}

fn doc_node(s: &Session, id: u64) -> figcraft_doc::Node {
    s.doc().unwrap().node(figcraft_doc::NodeId(id)).unwrap().clone()
}

fn id_of(s: &mut Session, name: &str) -> u64 {
    *ids(s, name).first().unwrap_or_else(|| panic!("no layer named {name}"))
}

fn doc_nodes_named(s: &mut Session, name: &str) -> Vec<figcraft_doc::Node> {
    ids(s, name).into_iter().map(|id| doc_node(s, id)).collect()
}

#[test]
fn css_grid_becomes_a_grid_layout_with_tracks_gaps_and_spans() {
    use figcraft_doc::GridTrack;
    let mut s = Session::new();
    let r = ex(
        &mut s,
        "<style>.g div{background:#eee}.g{display:grid;grid-template-columns:100px 1fr repeat(2, minmax(0, 2fr)) auto;gap:8px 16px;padding:10px;width:800px}
          .a{grid-column:span 2}.b{grid-column:2 / 4;grid-row:2}.c{grid-column:1 / span 2}.d{grid-column:5}</style>
         <div class=g><div class=a>A</div><div class=b>B</div><div class=c>C</div><div class=d>D</div></div>",
    );
    assert_eq!(warnings(&r), ["`minmax()` grid tracks use their maximum"], "{r}");
    let grid = get(&mut s, "div.g");
    assert_eq!(
        (grid["layout"]["mode"].clone(), grid["layout"]["gap"].clone(), grid["layout"]["crossGap"].clone()),
        (json!("grid"), json!(16.0), json!(8.0)),
        "{grid}"
    );
    let gid = id_of(&mut s, "div.g");
    let al = doc_node(&s, gid).layout.unwrap();
    assert_eq!(al.column_tracks, vec![GridTrack::fixed(100.0), GridTrack::fill(1.0), GridTrack::fill(2.0), GridTrack::fill(2.0), GridTrack::hug()]);
    assert_eq!(al.grid_columns, 5);
    let cl = |s: &mut Session, name: &str| {
        let id = id_of(s, name);
        doc_node(s, id).child_layout
    };
    let a = cl(&mut s, "div.a");
    assert_eq!((a.grid_column, a.grid_column_span, a.grid_row), (None, 2, None));
    let b = cl(&mut s, "div.b");
    assert_eq!((b.grid_column, b.grid_column_span, b.grid_row), (Some(1), 2, Some(1)));
    let c = cl(&mut s, "div.c");
    assert_eq!((c.grid_column, c.grid_column_span), (Some(0), 2));
    assert_eq!(cl(&mut s, "div.d").grid_column, Some(4));
}

#[test]
fn grid_items_fill_their_cells() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<div style='display:grid;grid-template-columns:100px 1fr 1fr;width:400px;height:60px'><style>div div{background:#eee}</style><div class=a>a</div><div class=b>b</div><div class=c>c</div></div>",
    );
    let at = |s: &mut Session, name: &str| {
        let n = get(s, name);
        (n["x"].as_f64().unwrap(), n["width"].as_f64().unwrap(), n["height"].as_f64().unwrap())
    };
    assert_eq!(at(&mut s, "div.a"), (0.0, 100.0, 60.0));
    assert_eq!(at(&mut s, "div.b"), (100.0, 150.0, 60.0));
    assert_eq!(at(&mut s, "div.c"), (250.0, 150.0, 60.0));
    // Items that opt out of stretching keep their own size.
    ex(
        &mut s,
        "<div style='display:grid;grid-template-columns:1fr 1fr;width:400px;justify-items:center'><div style='width:50px;height:20px;background:#f00' id=k></div></div>",
    );
    let k = get(&mut s, "div#k");
    assert_eq!((k["x"].clone(), k["width"].clone()), (json!(75.0), json!(50.0)), "centred in a 200 px cell: {k}");
}

#[test]
fn grid_values_that_are_not_understood_warn_and_stay_bounded() {
    let mut s = Session::new();
    let r = ex(
        &mut s,
        "<div style='display:grid;grid-template-columns:repeat(auto-fill, 100px) 5% bogus;grid-template-rows:repeat(99999, 1fr)'><p style='grid-column:span 0 / -1;grid-row:foo'>x</p></div>
         <div style='display:grid;grid-template-columns:repeat(0,1fr) minmax(;'><i>y</i></div>
         <div style='display:grid;grid-template-columns:none;grid-template-rows:200px auto'><i>z</i></div>",
    );
    let w = warnings(&r).join("\n");
    for needle in ["auto-fill", "5%", "bogus", "grid-column", "grid-row"] {
        assert!(w.contains(needle), "no warning mentions {needle}: {w}");
    }
    for g in doc_nodes_named(&mut s, "div") {
        let al = g.layout.unwrap();
        assert!((1..=100).contains(&al.grid_columns) && al.grid_rows <= 100, "{} x {}", al.grid_columns, al.grid_rows);
    }
}

#[test]
fn absolute_children_of_a_positioned_parent_leave_the_flow_at_their_offsets() {
    let mut s = Session::new();
    let r = ex(
        &mut s,
        "<div class=box style='position:relative;width:300px;height:200px'>
           <div class=tl style='position:absolute;top:10px;left:20px;width:50px;height:30px'>t</div>
           <div class=br style='position:absolute;right:10px;bottom:5px;width:40px;height:20px'>b</div>
           <div class=wide style='position:absolute;left:10px;right:10px;top:0;height:10px'>w</div>
           <div class=full style='position:absolute;inset:0'></div>
           <p class=flow style='margin:0'>flow</p></div>",
    );
    assert!(warnings(&r).is_empty(), "{r}");
    let pos = |s: &mut Session, name: &str| {
        let n = get(s, name);
        (n["absolute"].clone(), n["x"].as_f64().unwrap(), n["y"].as_f64().unwrap(), n["width"].as_f64().unwrap(), n["height"].as_f64().unwrap())
    };
    assert_eq!(pos(&mut s, "div.tl"), (json!(true), 20.0, 10.0, 50.0, 30.0));
    assert_eq!(pos(&mut s, "div.br"), (json!(true), 250.0, 175.0, 40.0, 20.0), "anchored to the bottom right corner");
    assert_eq!(pos(&mut s, "div.wide"), (json!(true), 10.0, 0.0, 280.0, 10.0), "left and right give the width");
    assert_eq!(pos(&mut s, "div.full"), (json!(true), 0.0, 0.0, 300.0, 200.0), "inset: 0 covers the parent");
    let br = get(&mut s, "div.br");
    assert_eq!((br["constraints"]["horizontal"].clone(), br["constraints"]["vertical"].clone()), (json!("right"), json!("bottom")));
    let wide = get(&mut s, "div.wide");
    assert_eq!((wide["constraints"]["horizontal"].clone(), wide["constraints"]["vertical"].clone()), (json!("leftRight"), json!("top")));
    let flow = get(&mut s, "flow");
    assert!(flow.get("absolute").is_none(), "{flow}");
    assert_eq!(flow["y"], json!(0.0), "the others do not leave room for absolute boxes: {flow}");
}

#[test]
fn an_absolute_box_is_placed_from_the_size_a_hugging_parent_ends_up_with() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<div class=p style='position:relative;padding:10px;display:inline-block'><span class=badge style='position:absolute;right:0;top:-4px;display:block;width:20px;height:20px;background:#f00'></span>hello world</div>",
    );
    let p = get(&mut s, "div.p");
    let badge = get(&mut s, "span.badge");
    let (pw, bw) = (p["width"].as_f64().unwrap(), badge["width"].as_f64().unwrap());
    assert!(pw > 20.0, "the parent hugs its text, not the badge: {p}");
    assert_eq!((badge["x"].as_f64().unwrap(), badge["y"].clone(), badge["absolute"].clone()), (pw - bw, json!(-4.0), json!(true)), "{badge}");
}

#[test]
fn absolute_without_a_positioned_parent_and_relative_offsets_stay_warnings() {
    let mut s = Session::new();
    let r = ex(
        &mut s,
        "<div><p class=a style='position:absolute;top:5px'>a</p></div><div style='position:relative'><p class=b style='position:relative;top:5px'>b</p><p class=c style='position:fixed'>c</p></div>",
    );
    let w = warnings(&r).join("\n");
    assert!(w.contains("absolute without a positioned parent"), "{w}");
    assert!(w.contains("position: relative offsets"), "{w}");
    for name in ["a", "b", "c"] {
        assert!(get(&mut s, name).get("absolute").is_none(), "{name} stays in the flow");
    }
}

#[test]
fn flex_rows_stretch_their_boxes_to_the_tallest_even_when_the_row_hugs() {
    let mut s = Session::new();
    ex(
        &mut s,
        "<div class=row style='display:flex;gap:10px;width:400px'>
           <div class=a style='flex:1;padding:8px;border:1px solid #000'>short</div>
           <div class=b style='flex:1;padding:8px;border:1px solid #000'><div style='height:80px;background:#ccc'></div></div></div>",
    );
    let (a, b, row) = (get(&mut s, "div.a"), get(&mut s, "div.b"), get(&mut s, "div.row"));
    assert!(b["height"].as_f64().unwrap() > 40.0, "{b}");
    assert_eq!(a["height"], b["height"], "stretched to the tallest child: {a}");
    assert_eq!(a["sizing"]["vertical"], json!("fill"), "{a}");
    assert_eq!((row["sizing"]["vertical"].clone(), row["height"].clone()), (json!("fixed"), b["height"].clone()), "{row}");

    ex(
        &mut s,
        "<div class=row2 style='display:flex;align-items:flex-start;width:400px'>
           <div class=a2 style='flex:1;padding:8px;border:1px solid #000'>short</div>
           <div class=b2 style='flex:1;padding:8px;border:1px solid #000'><div style='height:80px;background:#ccc'></div></div></div>",
    );
    let (a, b, row) = (get(&mut s, "div.a2"), get(&mut s, "div.b2"), get(&mut s, "div.row2"));
    assert!(a["height"].as_f64().unwrap() < b["height"].as_f64().unwrap(), "flex-start does not stretch: {a}");
    assert_eq!((a["sizing"]["vertical"].clone(), row["sizing"]["vertical"].clone()), (json!("hug"), json!("hug")));
}

#[test]
fn media_queries_follow_the_viewport_width_option() {
    let html = "<style>.r{display:flex;flex-direction:row} @media (max-width: 600px) { .r{flex-direction:column} } @media (min-width: 601px) and (max-width: 2000px) { .r{gap:7px} }</style><div class=r><i>a</i><i>b</i></div>";
    let mut s = Session::new();
    let narrow = s.execute("import.html", &json!({"html": html, "width": 600})).unwrap();
    assert!(warnings(&narrow).is_empty(), "{narrow}");
    let r = get(&mut s, "div.r");
    assert_eq!((r["layout"]["mode"].clone(), r["layout"]["gap"].clone()), (json!("vertical"), json!(0.0)), "{r}");
    s.execute("import.html", &json!({"html": html, "width": 601})).unwrap();
    let wide = doc_nodes_named(&mut s, "div.r");
    let gapped = wide.iter().filter(|n| n.layout.as_ref().is_some_and(|l| l.gap == 7.0)).count();
    assert_eq!(gapped, 1, "601 px takes the second query and not the first");
}

#[test]
fn first_last_and_nth_child_select_by_place() {
    let mut s = Session::new();
    let r = ex(
        &mut s,
        "<style>li:first-child{color:#ff0000} li:last-child{color:#00ff00} li:nth-child(2){color:#0000ff} li:nth-child(2n){color:#000000}</style>
         <ul><li>one</li><li>two</li><li>three</li></ul>",
    );
    let colour = |s: &mut Session, t: &str| get(s, t)["fills"][0]["color"].clone();
    assert_eq!(colour(&mut s, "one"), json!("#FF0000"));
    assert_eq!(colour(&mut s, "two"), json!("#0000FF"));
    assert_eq!(colour(&mut s, "three"), json!("#00FF00"));
    assert!(warnings(&r).iter().any(|w| w.contains("nth-child(2n)")), "only plain numbers: {r}");
}
