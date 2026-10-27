//! `cargo test --release -p figcraft-doc size_report -- --ignored --nocapture` prints the size of
//! `Node` and its field groups (memory work: keep cold payloads small or boxed).

use super::node::*;
use super::props::*;
use std::mem::size_of as s;

#[test]
#[ignore = "prints a report"]
fn size_report() {
    println!("SIZE Node={}", s::<Node>());
    println!("SIZE NodeKind={} TextData={}", s::<NodeKind>(), s::<crate::text::TextData>());
    println!("SIZE StrokeProps={} StyleRefs={} ChildLayout={}", s::<StrokeProps>(), s::<crate::styles::StyleRefs>(), s::<ChildLayout>());
    println!("SIZE Option<AutoLayout>={} Corners={} Constraints={} Affine={}", s::<Option<AutoLayout>>(), s::<Corners>(), s::<Constraints>(), s::<figcraft_geom::Affine>());
    println!("SIZE ScrollPosition={} Overflow={} Option<BoardItem>={}", s::<crate::proto::ScrollPosition>(), s::<crate::proto::Overflow>(), s::<Option<crate::BoardItem>>());
    println!("SIZE Vec={} BTreeMap={} BTreeSet={} String={}", s::<Vec<u8>>(), s::<std::collections::BTreeMap<String, String>>(), s::<std::collections::BTreeSet<String>>(), s::<String>());
}

