//! A result no client could read is replaced by an error that says how to ask for less.

use serde_json::json;

use crate::tools::{ToolResult, oversize};

#[test]
fn a_huge_text_or_image_result_becomes_an_error_that_names_the_ways_to_narrow_it() {
    let small = ToolResult::text("x".repeat(1000));
    assert_eq!(oversize("get_node", &small), None);
    let big = ToolResult::text("x".repeat((8 << 20) + 1));
    let why = oversize("get_node", &big).unwrap();
    assert!(why.contains("get_node") && why.contains("9 MiB of text") && why.contains("lower `limit`"), "{why}");
    let image = ToolResult { content: vec![json!({"type": "image", "data": "A".repeat((16 << 20) + 1), "mimeType": "image/png"})], is_error: false };
    assert!(oversize("screenshot", &image).unwrap().contains("image data"));
    // Text at the limit is still fine, and an image does not count as text.
    let edge = ToolResult::text("x".repeat(8 << 20));
    assert_eq!(oversize("get_node", &edge), None);
}
