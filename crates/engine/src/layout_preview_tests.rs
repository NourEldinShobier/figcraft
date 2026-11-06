//! `layout.previewWidths`: a frame laid out at other widths on a copy of the document. The preview
//! must match what resizing the frame really does, and leave the design, selection and history alone.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::Session;
use crate::params::id_of;
use figcraft_doc::NodeId;

type Boxes = Vec<(f64, f64, f64, f64)>;

fn id(v: &Value) -> NodeId {
    id_of(&v["id"]).unwrap()
}

/// A wrapping frame 1440 wide at `y`: hug height, gap 20, padding 0, five 300 x 100 children.
fn wrap_frame(s: &mut Session, y: f64) -> NodeId {
    let f = id(&s.execute("frame.create", &json!({"x": 0, "y": y, "width": 1440, "height": 100})).unwrap());
    let props = json!({"layout": "horizontal", "wrap": true, "padding": 0, "gap": 20, "crossGap": 20, "fills": [], "sizingV": "hug"});
    s.execute("node.set", &json!({"id": f, "props": props})).unwrap();
    for _ in 0..5 {
        s.execute("shape.rectangle", &json!({"parent": f, "x": 0, "y": 0, "width": 300, "height": 100})).unwrap();
    }
    f
}

fn size(s: &Session, f: NodeId) -> (f64, f64) {
    let n = s.doc().unwrap().node(f).unwrap();
    (n.width, n.height)
}

fn children(s: &Session, f: NodeId) -> Vec<NodeId> {
    s.doc().unwrap().node(f).unwrap().children.clone()
}

/// What the document holds for the children of `f`: x, y, width, height.
fn boxes(s: &Session, f: NodeId) -> Boxes {
    let doc = s.doc().unwrap();
    doc.node(f)
        .unwrap()
        .children
        .iter()
        .map(|c| {
            let n = doc.node(*c).unwrap();
            let t = n.transform.as_coeffs();
            (t[4], t[5], n.width, n.height)
        })
        .collect()
}

/// What a preview lists for its children.
fn listed(preview: &Value) -> Boxes {
    let num = |c: &Value, k: &str| c[k].as_f64().unwrap();
    preview["children"].as_array().unwrap().iter().map(|c| (num(c, "x"), num(c, "y"), num(c, "width"), num(c, "height"))).collect()
}

fn previews(s: &mut Session, params: Value) -> Vec<Value> {
    s.execute("layout.previewWidths", &params).unwrap()["previews"].as_array().unwrap().clone()
}

/// Everything a read-only command must leave alone.
fn state(s: &mut Session) -> Value {
    let history = s.execute("history.list", &json!({})).unwrap();
    let status = s.execute("doc.status", &json!({})).unwrap();
    json!({"total": history["total"], "applied": history["applied"], "hash": history["hash"], "status": status, "selection": s.selection()})
}

#[test]
fn previews_at_375_768_and_1440_count_the_rows_and_heights() {
    let mut s = Session::new();
    let f = wrap_frame(&mut s, 0.0);
    assert_eq!(size(&s, f), (1440.0, 220.0));
    let p = previews(&mut s, json!({"id": f, "widths": [375, 768, 1440]}));
    let got: Vec<(f64, f64, u64)> =
        p.iter().map(|v| (v["width"].as_f64().unwrap(), v["height"].as_f64().unwrap(), v["rows"].as_u64().unwrap())).collect();
    assert_eq!(got, [(375.0, 580.0, 5), (768.0, 340.0, 3), (1440.0, 220.0, 2)]);
    assert_eq!(p.iter().map(|v| v["childCount"].as_u64().unwrap()).collect::<Vec<_>>(), [5, 5, 5]);
    assert_eq!(
        listed(&p[0]),
        [(0.0, 0.0, 300.0, 100.0), (0.0, 120.0, 300.0, 100.0), (0.0, 240.0, 300.0, 100.0), (0.0, 360.0, 300.0, 100.0), (0.0, 480.0, 300.0, 100.0)]
    );
    assert_eq!(
        listed(&p[1]),
        [(0.0, 0.0, 300.0, 100.0), (320.0, 0.0, 300.0, 100.0), (0.0, 120.0, 300.0, 100.0), (320.0, 120.0, 300.0, 100.0), (0.0, 240.0, 300.0, 100.0)]
    );
    assert_eq!(
        listed(&p[2]),
        [(0.0, 0.0, 300.0, 100.0), (320.0, 0.0, 300.0, 100.0), (640.0, 0.0, 300.0, 100.0), (960.0, 0.0, 300.0, 100.0), (0.0, 120.0, 300.0, 100.0)]
    );
    assert!(p.iter().all(|v| v["overflowing"] == json!([])));
    // The source frame is still what it was.
    assert_eq!(size(&s, f), (1440.0, 220.0));
}

#[test]
fn a_preview_matches_what_resizing_the_frame_really_lays_out() {
    let mut s = Session::new();
    let f = wrap_frame(&mut s, 0.0);
    let p = previews(&mut s, json!({"id": f, "widths": [375, 600, 768, 1000, 1440]}));
    for (i, w) in [375.0, 600.0, 768.0, 1000.0, 1440.0].into_iter().enumerate() {
        let copy = wrap_frame(&mut s, 3000.0);
        s.execute("node.set", &json!({"id": copy, "props": {"width": w}})).unwrap();
        let (cw, ch) = size(&s, copy);
        assert_eq!((p[i]["width"].as_f64().unwrap(), p[i]["height"].as_f64().unwrap()), (cw, ch), "frame size at {w}");
        assert_eq!(listed(&p[i]), boxes(&s, copy), "children at {w}");
    }
}

#[test]
fn a_preview_changes_nothing_not_even_with_renders() {
    let mut s = Session::new();
    let f = wrap_frame(&mut s, 0.0);
    s.set_selection(children(&s, f).into_iter().take(2).collect());
    let before = state(&mut s);
    let doc_before = s.doc().unwrap().clone();
    previews(&mut s, json!({"id": f, "widths": [375, 768], "heights": [900, 400], "render": true}));
    previews(&mut s, json!({"id": f, "widths": [200]}));
    assert_eq!(state(&mut s), before);
    assert!(*s.doc().unwrap() == doc_before, "the document is the same");
    assert_eq!(size(&s, f), (1440.0, 220.0));
    // The next undo is still the last real edit (the fifth rectangle), not a preview.
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(children(&s, f).len(), 4);
}

#[test]
fn renders_are_pngs_at_the_scale_asked_for() {
    let mut s = Session::new();
    let f = wrap_frame(&mut s, 0.0);
    let plain = previews(&mut s, json!({"id": f, "widths": [768]}));
    assert!(plain[0].get("data").is_none() && plain[0].get("format").is_none());
    let p = previews(&mut s, json!({"id": f, "widths": [768, 1440], "render": true, "scale": 0.5}));
    let dims: Vec<(u32, u32)> = p
        .iter()
        .map(|v| {
            assert_eq!(v["format"], json!("png"));
            let png = base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap()).unwrap();
            assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
            let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
            (be(16), be(20))
        })
        .collect();
    assert_eq!(dims, [(384, 170), (720, 110)]);
}

#[test]
fn heights_set_the_frame_height_and_overflowing_children_are_listed() {
    let mut s = Session::new();
    let f = wrap_frame(&mut s, 0.0);
    let kids = children(&s, f);
    let p = previews(&mut s, json!({"id": f, "widths": [375, 375], "heights": [300, 900]}));
    assert_eq!((p[0]["height"].as_f64().unwrap(), p[1]["height"].as_f64().unwrap()), (300.0, 900.0));
    // Rows end at 100, 220, 340, 460 and 580: the last three pass a 300 high frame.
    assert_eq!(p[0]["overflowing"], json!([kids[2], kids[3], kids[4]]));
    assert_eq!(p[1]["overflowing"], json!([]));
    // A frame narrower than its children pushes them all past the right edge.
    let narrow = previews(&mut s, json!({"id": f, "widths": [200]}));
    assert_eq!(narrow[0]["overflowing"], json!(kids));
    assert_eq!(size(&s, f), (1440.0, 220.0));
}

#[test]
fn only_wrapping_frames_report_rows() {
    let mut s = Session::new();
    let f = id(&s.execute("frame.create", &json!({"x": 0, "y": 0, "width": 200, "height": 100})).unwrap());
    s.execute("node.set", &json!({"id": f, "props": {"layout": "vertical", "padding": 0, "gap": 10}})).unwrap();
    s.execute("shape.rectangle", &json!({"parent": f, "x": 0, "y": 0, "width": 50, "height": 20})).unwrap();
    let p = previews(&mut s, json!({"id": f, "widths": [300]}));
    assert!(p[0].get("rows").is_none(), "{}", p[0]);
    assert_eq!(p[0]["width"], json!(300.0));
}

#[test]
fn junk_parameters_are_refused_and_change_nothing() {
    let mut s = Session::new();
    let f = wrap_frame(&mut s, 0.0);
    let plain = id(&s.execute("shape.rectangle", &json!({"x": 0, "y": 500, "width": 50, "height": 20})).unwrap());
    let before = state(&mut s);
    let thirteen: Vec<u32> = (1..=13).map(|n| n * 100).collect();
    let junk = [
        json!({"id": f}),
        json!({"id": f, "widths": []}),
        json!({"id": f, "widths": 375}),
        json!({"id": f, "widths": [0]}),
        json!({"id": f, "widths": [-5]}),
        json!({"id": f, "widths": [20001]}),
        json!({"id": f, "widths": [f64::NAN]}),
        json!({"id": f, "widths": ["wide"]}),
        json!({"id": f, "widths": [true]}),
        json!({"id": f, "widths": thirteen}),
        json!({"id": f, "widths": [375, 768], "heights": [500]}),
        json!({"id": f, "widths": [375], "heights": [0]}),
        json!({"id": f, "widths": [375], "heights": 500}),
        json!({"id": f, "widths": [375], "render": "maybe"}),
        json!({"id": f, "widths": [375], "scale": 0}),
        json!({"id": f, "widths": [375], "sizes": [1]}),
        json!({"id": plain, "widths": [375]}),
        json!({"id": 999_999, "widths": [375]}),
        json!({"widths": [375]}),
    ];
    for params in junk {
        assert!(s.execute("layout.previewWidths", &params).is_err(), "{params} was accepted");
    }
    // Twelve is the most.
    let twelve: Vec<u32> = (1..=12).map(|n| n * 100).collect();
    assert_eq!(previews(&mut s, json!({"id": f, "widths": twelve})).len(), 12);
    assert_eq!(state(&mut s), before);
}
