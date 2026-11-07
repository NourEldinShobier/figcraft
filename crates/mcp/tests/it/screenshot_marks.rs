//! `screenshot` with `marks`: badges on the image and a legend that maps them to layers.

use crate::common::Client;
use figcraft_mcp::TOOL_NAMES;
use serde_json::{Value, json};

/// The image block of a tool result decoded, and the JSON text block.
fn shot(c: &mut Client, args: Value) -> (figcraft_render::Pixmap, Value) {
    let r = c.call("screenshot", args.clone());
    assert_eq!(r["isError"], false, "{r}");
    let blocks = r["content"].as_array().unwrap();
    let data = blocks.iter().find_map(|b| b.get("data").and_then(Value::as_str)).unwrap();
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
    let text = blocks.iter().find_map(|b| b.get("text").and_then(Value::as_str)).unwrap();
    (figcraft_render::images::decode(&png).unwrap(), serde_json::from_str(text).unwrap())
}

fn px(p: &figcraft_render::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let i = (y * p.width() + x) as usize * 4;
    [p.data()[i], p.data()[i + 1], p.data()[i + 2], p.data()[i + 3]]
}

/// Dark box pixels and white digit pixels in the `size` square at (`x`, `y`).
fn badge_at(p: &figcraft_render::Pixmap, x: u32, y: u32, size: u32) -> bool {
    let all: Vec<[u8; 4]> = (0..size).flat_map(|dy| (0..size).map(move |dx| px(p, x + dx, y + dy))).collect();
    all.iter().any(|c| c[0] > 250 && c[1] > 250 && c[2] > 250) && all.iter().any(|c| c[0] < 110 && c[1] < 110 && c[2] < 110)
}

fn design(c: &mut Client) -> (Value, Value, Value) {
    let f = c.create(json!({"type": "frame", "name": "Screen", "width": 400, "height": 300, "fills": [{"type": "solid", "color": "#FF0000"}]}));
    let a = c.create(json!({"type": "rectangle", "name": "Red", "parent": f, "x": 200, "y": 150, "width": 100, "height": 100, "fills": [{"type": "solid", "color": "#FF0000"}]}));
    let b = c.create(json!({"type": "rectangle", "name": "Blue", "parent": f, "x": 200, "y": 150, "width": 100, "height": 100, "fills": [{"type": "solid", "color": "#FF0000"}]}));
    (f, a, b)
}

#[test]
fn marks_draw_badges_at_layer_corners_and_list_them() {
    let mut c = Client::new();
    let (f, a, b) = design(&mut c);
    let (plain, _) = shot(&mut c, json!({"ids": [f], "scale": 1}));
    assert_eq!(px(&plain, 205, 155), [255, 0, 0, 255]);
    assert!(!badge_at(&plain, 200, 150, 14));

    let (img, info) = shot(&mut c, json!({"ids": [f], "scale": 1, "marks": true}));
    assert_eq!((img.width(), img.height()), (plain.width(), plain.height()));
    let badges = info["marks"]["badges"].as_array().unwrap();
    assert_eq!(badges.len(), 3, "{info}");
    assert_eq!(badges[0], json!({"badge": 1, "id": f, "name": "Screen"}));
    assert_eq!(badges[1]["id"], a);
    assert_eq!(badges[2]["id"], b);
    // The frame's badge is in the image corner; the two children share a corner, so the second is nudged.
    assert!(badge_at(&img, 0, 0, 14));
    assert!(badge_at(&img, 200, 150, 14));
    let second_at_corner = (0..2).filter(|i| badge_at(&img, 200, 150 + i * 14, 14)).count();
    assert_eq!(second_at_corner, 2, "badges 2 and 3 do not overlap");
}

#[test]
fn mark_depth_style_and_limit_shape_the_legend() {
    let mut c = Client::new();
    let (f, _a, _b) = design(&mut c);
    let (_, top) = shot(&mut c, json!({"ids": [f], "scale": 1, "marks": true, "markDepth": 0}));
    assert_eq!(top["marks"]["badges"].as_array().unwrap().len(), 1);

    let (_, by_id) = shot(&mut c, json!({"ids": [f], "scale": 1, "marks": {"style": "id"}}));
    assert_eq!(by_id["marks"]["badges"][0]["badge"], f);

    let (_, capped) = shot(&mut c, json!({"ids": [f], "scale": 1, "marks": {"limit": 2}}));
    assert_eq!(capped["marks"]["badges"].as_array().unwrap().len(), 2);
    assert!(capped["marks"]["truncated"].as_str().unwrap().contains("2"), "{capped}");

    // The whole page: its top-level layers are marked.
    let (_, page) = shot(&mut c, json!({"marks": true, "markDepth": 0}));
    assert_eq!(page["marks"]["badges"][0]["id"], f);

    assert!(!c.err("screenshot", json!({"marks": "yes"})).is_empty());
    assert!(!c.err("screenshot", json!({"marks": {"style": "bogus"}})).is_empty());
}

#[test]
fn badges_stay_inside_a_tiny_image() {
    let mut c = Client::new();
    let f = c.create(json!({"type": "frame", "name": "Tiny", "width": 12, "height": 12}));
    for i in 0..4 {
        c.create(json!({"type": "rectangle", "parent": f, "x": i, "y": i, "width": 5, "height": 5}));
    }
    let (img, info) = shot(&mut c, json!({"ids": [f], "scale": 1, "marks": true}));
    assert_eq!((img.width(), img.height()), (12, 12));
    assert_eq!(info["marks"]["badges"].as_array().unwrap().len(), 5);
}

#[test]
fn marks_are_documented() {
    assert!(TOOL_NAMES.contains(&"screenshot"));
    let doc = include_str!("../../../../docs/mcp.md");
    assert!(doc.contains("`marks`") && doc.contains("`markDepth`"));
}
