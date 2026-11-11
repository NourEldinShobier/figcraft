//! `export.assets`: every layer with saved export settings under a root, written in one call with a manifest.

use serde_json::{Value, json};

use crate::Session;

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("figcraft-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rect(s: &mut Session, name: &str, x: f64) -> Value {
    ex(s, "node.create", json!({"type": "rectangle", "x": x, "y": 0, "width": 24, "height": 24, "name": name}))["id"].clone()
}

#[test]
fn writes_layers_with_export_settings_and_a_manifest() {
    let mut s = Session::new();
    let (a, b, plain) = (rect(&mut s, "logo", 0.0), rect(&mut s, "icon/star", 40.0), rect(&mut s, "plain", 80.0));
    ex(&mut s, "export.add", json!({"ids": [a]}));
    ex(&mut s, "export.add", json!({"ids": [b]}));
    let _ = plain;
    let dir = temp_dir("ok");
    let r = ex(&mut s, "export.assets", json!({"dir": dir.to_string_lossy()}));
    assert_eq!(r["written"], 2, "{r}");
    let names: Vec<&str> = r["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["logo.png", "icon/star.png"]);
    assert_eq!((r["files"][0]["format"].clone(), r["files"][0]["scale"].clone()), (json!("png"), json!(1.0)));
    assert!(dir.join("logo.png").exists() && dir.join("icon/star.png").exists());
    assert!(!dir.join("plain.png").exists());
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["files"][1]["name"], "icon/star.png");
    assert_eq!(manifest["files"][0]["bytes"], r["files"][0]["bytes"]);
    // A root limits it to its subtree.
    let dir2 = temp_dir("root");
    let r = ex(&mut s, "export.assets", json!({"dir": dir2.to_string_lossy(), "root": a}));
    assert_eq!(r["written"], 1);
    let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&dir2));
}

#[test]
fn existing_files_need_overwrite_and_nothing_is_written_without_it() {
    let mut s = Session::new();
    let a = rect(&mut s, "logo", 0.0);
    ex(&mut s, "export.add", json!({"ids": [a]}));
    let dir = temp_dir("overwrite");
    std::fs::write(dir.join("logo.png"), b"mine").unwrap();
    let e = s.execute("export.assets", &json!({"dir": dir.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("logo.png") && e.contains("overwrite"), "{e}");
    assert_eq!(std::fs::read(dir.join("logo.png")).unwrap(), b"mine");
    assert!(!dir.join("manifest.json").exists());
    ex(&mut s, "export.assets", json!({"dir": dir.to_string_lossy(), "overwrite": true}));
    assert!(std::fs::read(dir.join("logo.png")).unwrap().starts_with(b"\x89PNG"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_export_settings_junk_and_limits_are_errors() {
    let mut s = Session::new();
    let a = rect(&mut s, "logo", 0.0);
    let dir = temp_dir("errors");
    let d = dir.to_string_lossy().to_string();
    let e = s.execute("export.assets", &json!({"dir": d})).unwrap_err().to_string();
    assert!(e.contains("no layer"), "{e}");
    ex(&mut s, "export.add", json!({"ids": [a]}));
    ex(&mut s, "export.add", json!({"ids": [a]}));
    for p in [
        json!({}),
        json!({"dir": ""}),
        json!({"dir": 5}),
        json!({"dir": d, "overwrite": ["x"]}),
        json!({"dir": d, "limit": 0}),
        json!({"dir": d, "limit": 201}),
        json!({"dir": d, "limit": "x"}),
        json!({"dir": d, "root": 99999}),
        json!({"dir": d, "root": "nope"}),
        json!({"dir": d, "formats": ["png"]}),
        // Two files, a limit of one.
        json!({"dir": d, "limit": 1}),
    ] {
        assert!(s.execute("export.assets", &p).is_err(), "{p}");
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refuses_a_folder_outside_the_write_root() {
    let mut s = Session::new();
    let a = rect(&mut s, "logo", 0.0);
    ex(&mut s, "export.add", json!({"ids": [a]}));
    let (inside, outside) = (temp_dir("inside"), temp_dir("outside"));
    let roots = std::sync::Arc::new(crate::file_access::Roots::new(&[], &[inside.to_string_lossy().into_owned()]).unwrap());
    crate::file_access::confined(Some(&roots), || {
        let e = s.execute("export.assets", &json!({"dir": outside.to_string_lossy()})).unwrap_err().to_string();
        assert!(e.contains("automation path rejected"), "{e}");
        // `..` out of the root is refused too.
        let sneaky = inside.join("..").join(outside.file_name().unwrap());
        assert!(s.execute("export.assets", &json!({"dir": sneaky.to_string_lossy()})).is_err());
        ex(&mut s, "export.assets", json!({"dir": inside.join("out").to_string_lossy()}));
    });
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(inside.join("out/logo.png").exists());
    let _ = (std::fs::remove_dir_all(&inside), std::fs::remove_dir_all(&outside));
}
