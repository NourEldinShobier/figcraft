//! `file.exportMany`: every layer in every format and scale without export settings, and the SVG icon preset.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn rect(s: &mut Session, name: &str, x: f64, fill: &str) -> Value {
    let id = ex(s, "node.create", json!({"type": "rectangle", "x": x, "y": 5, "width": 24, "height": 24, "name": name}))["id"].clone();
    ex(s, "node.set", json!({"ids": [id], "props": {"fill": fill}}));
    id
}

fn png_width(data: &Value) -> u32 {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_str().unwrap()).unwrap();
    u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]])
}

#[test]
fn every_layer_comes_out_in_every_format_and_scale() {
    let mut s = Session::new();
    let a = rect(&mut s, "folder", 0.0, "#FF0000");
    let b = rect(&mut s, "file", 40.0, "#00FF00");
    let r = ex(&mut s, "file.exportMany", json!({"ids": [a, b], "formats": ["png", "svg"], "scales": [1, 2]}));
    let names: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["folder@1x.png", "folder@2x.png", "folder.svg", "file@1x.png", "file@2x.png", "file.svg"]);
    assert_eq!(png_width(&r["data"]["folder@1x.png"]), 24);
    assert_eq!(png_width(&r["data"]["file@2x.png"]), 48);
    assert!(r["data"]["file.svg"].is_string());
    assert_eq!(r["files"][1]["scale"], 2.0);
}

#[test]
fn naming_template_single_scale_and_repeated_names() {
    let mut s = Session::new();
    let a = rect(&mut s, "dup", 0.0, "#FF0000");
    let b = rect(&mut s, "dup", 40.0, "#FF0000");
    let r = ex(&mut s, "file.exportMany", json!({"ids": [a, b], "formats": ["svg"]}));
    let names: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["dup.svg", "dup 2.svg"]);
    let r = ex(&mut s, "file.exportMany", json!({"ids": [a], "scales": [1.5], "naming": "icons/{name}-{scale}x-{format}.{ext}"}));
    assert_eq!(r["files"][0]["name"], "icons/dup-1.5x-png.png");
}

#[test]
fn files_go_to_a_folder_with_dir() {
    let mut s = Session::new();
    let a = rect(&mut s, "one", 0.0, "#FF0000");
    let dir = std::env::temp_dir().join(format!("figcraft-export-many-{}", std::process::id()));
    let r = ex(&mut s, "file.exportMany", json!({"ids": [a], "formats": ["png"], "scales": [1, 2], "dir": dir.to_string_lossy()}));
    assert!(r.get("data").is_none());
    assert!(dir.join("one@1x.png").is_file() && dir.join("one@2x.png").is_file());
    assert!(r["files"][0]["path"].as_str().unwrap().ends_with("one@1x.png"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn junk_parameters_are_refused() {
    let mut s = Session::new();
    rect(&mut s, "x", 0.0, "#FF0000");
    for p in [
        json!({"formats": ["gif"]}),
        json!({"formats": []}),
        json!({"formats": "png"}),
        json!({"scales": [0]}),
        json!({"scales": ["2x"]}),
        json!({"scales": [1e9]}),
        json!({"naming": 3}),
        json!({"iconPreset": [1]}),
        json!({"currentColor": true}),
    ] {
        assert!(s.execute("file.exportMany", &p).is_err(), "{p}");
    }
}

#[test]
fn the_icon_preset_bakes_transforms_and_uses_the_frames_box() {
    let mut s = Session::new();
    let a = rect(&mut s, "sq", 10.0, "#FF0000");
    let svg = |s: &mut Session, p: Value| {
        let r = ex(s, "file.export", p);
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(r["data"].as_str().unwrap()).unwrap()).unwrap()
    };
    let plain = svg(&mut s, json!({"ids": [a], "format": "svg"}));
    assert!(plain.contains("transform=\"matrix("), "{plain}");
    let icon = svg(&mut s, json!({"ids": [a], "format": "svg", "iconPreset": true, "currentColor": true}));
    assert!(icon.contains("viewBox=\"0 0 24 24\""), "{icon}");
    assert!(!icon.contains("transform="), "{icon}");
    assert!(icon.contains("fill=\"currentColor\""), "{icon}");
    assert!(!icon.contains("<g"), "{icon}");
}
