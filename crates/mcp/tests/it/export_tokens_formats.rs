//! `export_tokens` formats beyond dtcg/css/tailwind/json: styles, Style Dictionary, resolver and native output.

use crate::common::Client;
use serde_json::json;

fn themed(c: &mut Client) {
    c.run("variable.createCollection", json!({"name": "Theme", "modes": ["Light", "Dark"]}));
    c.run("variable.create", json!({"collection": "Theme", "name": "brand", "type": "color", "values": {"Light": "#0000FF", "Dark": "#FFFFFF"}}));
    c.run("variable.create", json!({"collection": "Theme", "name": "gap", "type": "number", "values": {"Light": 8, "Dark": 8}}));
}

#[test]
fn native_formats_come_back_as_text_or_files() {
    let mut c = Client::new();
    themed(&mut c);
    let swift = c.ok("export_tokens", json!({"format": "swift", "modes": ["Light"]}));
    assert!(swift.as_str().unwrap().contains("static let brand: Color = Color(.sRGB, red: 0, green: 0, blue: 1, opacity: 1)"), "{swift}");
    let kotlin = c.ok("export_tokens", json!({"format": "kotlin", "modes": ["Light"]}));
    assert!(kotlin.as_str().unwrap().contains("val gap = 8.dp"), "{kotlin}");
    let android = c.ok("export_tokens", json!({"format": "android"}));
    assert_eq!(android["files"][0]["name"], "values/colors.xml", "{android}");
    assert!(android["files"][2]["text"].as_str().unwrap().contains("#FFFFFFFF"), "{android}");
}

#[test]
fn style_dictionary_resolver_and_styles() {
    let mut c = Client::new();
    themed(&mut c);
    let sd = c.ok("export_tokens", json!({"format": "style-dictionary"}));
    assert_eq!(sd["files"][1]["tokens"]["brand"]["$value"], "#0000FF", "{sd}");
    let resolver = c.ok("export_tokens", json!({"format": "resolver"}));
    assert_eq!(resolver["files"][0]["name"], "tokens.resolver.json");
    assert!(resolver["files"][0]["text"].as_str().unwrap().contains("\"default\": \"Light\""), "{resolver}");
    assert!(c.err("export_tokens", json!({"format": "dtcg-styles"})).contains("no text, effect or paint styles"));
    c.run("style.create", json!({"type": "text", "name": "Body", "value": {"fontFamily": "Inter", "fontSize": 16}}));
    let styles = c.ok("export_tokens", json!({"format": "dtcg-styles"}));
    assert_eq!(styles["files"][0]["tokens"]["text"]["Body"]["$type"], "typography", "{styles}");
    assert!(c.err("export_tokens", json!({"format": "yaml"})).contains("format"));
}
