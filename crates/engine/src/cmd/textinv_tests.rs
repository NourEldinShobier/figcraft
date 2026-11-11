use figcraft_doc::NodeId;
use serde_json::{Value, json};

use crate::Session;

fn frame(s: &mut Session, name: &str, parent: Option<NodeId>) -> NodeId {
    figcraft_text::scan_system_fonts();
    let mut p = json!({"type": "frame", "name": name, "x": 0, "y": 0, "width": 400, "height": 300});
    if let Some(par) = parent {
        p["parent"] = json!(par);
    }
    let r = s.execute("node.create", &p).unwrap();
    NodeId(r["id"].as_u64().unwrap())
}

fn text(s: &mut Session, parent: NodeId, chars: &str, extra: Value) -> NodeId {
    let mut p = json!({"text": chars, "x": 0, "y": 0, "parent": parent, "width": 100});
    let fixed = extra.get("fixed").and_then(Value::as_f64);
    if let Some(w) = extra.get("width") {
        p["width"] = w.clone();
    }
    let r = s.execute("text.create", &p).unwrap();
    let id = NodeId(r["id"].as_u64().unwrap());
    if let Some(h) = fixed {
        s.execute("node.set", &json!({"ids": [id], "props": {"textAutoResize": "none", "height": h}})).unwrap();
    }
    id
}

fn chars(s: &Session, id: NodeId) -> String {
    s.doc().unwrap().node(id).unwrap().kind.text().unwrap().characters.clone()
}

#[test]
fn extract_lists_text_by_frame_with_path_box_and_style_flag() {
    let mut s = Session::new();
    let card = frame(&mut s, "Card", None);
    let inner = frame(&mut s, "Header", Some(card));
    let a = text(&mut s, inner, "Hello", json!({}));
    let b = text(&mut s, card, "Body copy", json!({}));
    s.execute("text.style", &json!({"id": b, "start": 0, "end": 4, "fontSize": 30})).unwrap();
    let r = s.execute("text.extract", &json!({})).unwrap();
    assert_eq!(r["total"], json!(2), "{r}");
    let t = &r["texts"];
    assert_eq!((t[0]["id"].clone(), t[0]["characters"].clone(), t[0]["path"].clone()), (json!(a), json!("Hello"), json!(["Card", "Header"])), "{r}");
    assert_eq!((t[0]["frame"].clone(), t[0]["styled"].clone(), t[0]["maxWidth"].clone()), (json!(inner), json!(false), json!(100.0)), "{r}");
    assert_eq!((t[1]["id"].clone(), t[1]["path"].clone(), t[1]["styled"].clone()), (json!(b), json!(["Card"]), json!(true)), "{r}");
    let one = s.execute("text.extract", &json!({"limit": 1})).unwrap();
    assert_eq!((one["listed"].clone(), one["truncated"].clone()), (json!(1), json!(true)), "{one}");
    let sub = s.execute("text.extract", &json!({"root": inner})).unwrap();
    assert_eq!(sub["total"], json!(1), "{sub}");
}

#[test]
fn apply_keeps_emoji_and_rtl_whole_and_reports_overflow() {
    let mut s = Session::new();
    let f = frame(&mut s, "F", None);
    let fixed = text(&mut s, f, "Hi", json!({"fixed": 20}));
    let grow = text(&mut s, f, "Hi", json!({}));
    let long = "مرحبا بالعالم 👩‍👩‍👧‍👦 héllo wörld ".repeat(6);
    let r = s.execute("text.apply", &json!({"changes": [{"id": fixed, "characters": long}, {"id": grow, "characters": "שלום 😀"}]})).unwrap();
    assert_eq!(r["applied"], json!(2), "{r}");
    assert_eq!(chars(&s, fixed), long);
    assert_eq!(chars(&s, grow), "שלום 😀");
    assert_eq!((r["results"][0]["fit"].clone(), r["results"][0]["was"].clone()), (json!("overflow"), json!("ok")), "{r}");
    assert_eq!(r["results"][1]["fit"], json!("ok"), "{r}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((chars(&s, fixed), chars(&s, grow)), ("Hi".to_string(), "Hi".to_string()), "one undo step");
}

#[test]
fn apply_refuses_styled_layers_unless_flatten_and_dry_run_changes_nothing() {
    let mut s = Session::new();
    let f = frame(&mut s, "F", None);
    let t = text(&mut s, f, "Hello world", json!({}));
    s.execute("text.style", &json!({"id": t, "start": 0, "end": 5, "fontSize": 40, "hyperlink": "https://example.com"})).unwrap();
    let ch = json!({"changes": [{"id": t, "characters": "Hallo Welt"}]});
    let r = s.execute("text.apply", &ch).unwrap();
    assert_eq!((r["applied"].clone(), r["refused"][0]["id"].clone()), (json!(0), json!(t)), "{r}");
    assert_eq!(r["refused"][0]["lost"]["links"], json!(1), "{r}");
    assert_eq!(chars(&s, t), "Hello world");
    let dry = s.execute("text.apply", &json!({"changes": ch["changes"], "flatten": true, "dryRun": true})).unwrap();
    assert_eq!(dry["applied"], json!(1), "{dry}");
    assert_eq!(chars(&s, t), "Hello world");
    let r = s.execute("text.apply", &json!({"changes": ch["changes"], "flatten": true})).unwrap();
    assert_eq!(r["results"][0]["lost"]["properties"], json!(["fontSize", "link"]), "{r}");
    assert_eq!(chars(&s, t), "Hallo Welt");
    assert!(s.doc().unwrap().node(t).unwrap().kind.text().unwrap().runs.is_empty());
    let same = s.execute("text.apply", &ch).unwrap();
    assert_eq!(same["unchanged"], json!(1), "{same}");
}

#[test]
fn pseudo_localize_accents_keeps_placeholders_and_restores_exactly() {
    let mut s = Session::new();
    let f = frame(&mut s, "F", None);
    let t = text(&mut s, f, "Hello {name}, <b>save</b> 100%\nsecond 😀", json!({}));
    s.execute("text.style", &json!({"id": t, "start": 0, "end": 5, "fontSize": 30})).unwrap();
    let before = s.doc().unwrap().node(t).unwrap().kind.text().unwrap().clone();
    let r = s.execute("text.pseudoLocalize", &json!({"mode": "accent"})).unwrap();
    assert_eq!(r["changed"], json!(1), "{r}");
    assert_eq!(chars(&s, t), "Ĥéļļö {name}, <b>šáṽé</b> 100%\nšéçöñð 😀");
    assert_eq!(s.doc().unwrap().node(t).unwrap().kind.text().unwrap().runs, before.runs, "runs stay on the same chars");
    // Running again starts from the original: no piling up.
    s.execute("text.pseudoLocalize", &json!({"mode": "both", "expand": 1.0})).unwrap();
    let c = chars(&s, t);
    assert!(c.starts_with("[Ĥéļļö {name}") && c.contains("]\n[šéçöñð 😀 lorem") && c.ends_with(']'), "{c}");
    let r = s.execute("text.pseudoLocalize", &json!({"mode": "restore"})).unwrap();
    assert_eq!(r["restored"], json!(1), "{r}");
    assert_eq!(*s.doc().unwrap().node(t).unwrap().kind.text().unwrap(), before);
    assert!(s.doc().unwrap().node(t).unwrap().plugin_data.is_empty());
}

#[test]
fn pseudo_localize_flags_overflow_skips_edited_layers_and_undoes_in_one_step() {
    let mut s = Session::new();
    let f = frame(&mut s, "F", None);
    let fixed = text(&mut s, f, "Save changes", json!({"fixed": 20, "width": 200}));
    let edited = text(&mut s, f, "Cancel", json!({}));
    let r = s.execute("text.pseudoLocalize", &json!({"expand": 2.0})).unwrap();
    assert_eq!(r["changed"], json!(2), "{r}");
    assert_eq!(r["overflow"][0]["id"], json!(fixed), "{r}");
    s.execute("text.apply", &json!({"changes": [{"id": edited, "characters": "Abbrechen"}]})).unwrap();
    let r = s.execute("text.pseudoLocalize", &json!({"mode": "restore"})).unwrap();
    assert_eq!((r["restored"].clone(), r["skipped"][0]["id"].clone()), (json!(1), json!(edited)), "{r}");
    assert_eq!((chars(&s, fixed), chars(&s, edited)), ("Save changes".to_string(), "Abbrechen".to_string()));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(chars(&s, fixed).starts_with('['), "undo of restore brings the pseudo text back");
}

#[test]
fn junk_parameters_are_errors_not_panics() {
    let mut s = Session::new();
    let f = frame(&mut s, "F", None);
    let t = text(&mut s, f, "x", json!({}));
    for junk in [
        json!({"changes": 5}),
        json!({"changes": [5]}),
        json!({"changes": [{"id": "x", "characters": "a"}]}),
        json!({"changes": [{"id": t, "characters": 3}]}),
        json!({"changes": [{"id": 99999, "characters": "a"}]}),
        json!({"changes": [{"id": f, "characters": "a"}]}),
        json!({"changes": [{"id": t, "characters": "a"}, {"id": t, "characters": "b"}]}),
        json!({"changes": [], "flatten": "perhaps"}),
        json!({}),
    ] {
        assert!(s.execute("text.apply", &junk).is_err(), "{junk}");
    }
    assert_eq!(chars(&s, t), "x");
    for junk in
        [json!({"mode": "bogus"}), json!({"expand": -1}), json!({"expand": "a"}), json!({"expand": 99}), json!({"root": 99999}), json!({"root": "x"})]
    {
        assert!(s.execute("text.pseudoLocalize", &junk).is_err(), "{junk}");
    }
    for junk in [json!({"limit": -1}), json!({"limit": 99999}), json!({"root": 99999}), json!({"page": 3})] {
        assert!(s.execute("text.extract", &junk).is_err(), "{junk}");
    }
    assert_eq!(s.execute("text.pseudoLocalize", &json!({"expand": 0})).unwrap()["changed"], json!(1));
    assert_eq!(chars(&s, t), "[ẋ]");
}
