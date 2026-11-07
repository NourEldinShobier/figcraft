//! The crate's integration tests, built as one test binary.

#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod a11y_tool;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod agent_comments;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod agent_tasks;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod attached;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::disallowed_methods)]
mod automation_roots;
#[allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod design_context_extras;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod diff_tools;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod export_hidden_roots;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod export_xml;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod family_tools;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod garbage;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod inspect_fields;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod inspect_response_limit;
mod lint_layout_tool;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod multi_doc_tools;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod overlay_anchor;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod protocol;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod screenshot_marks;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod screenshot_pixel_limit;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod selection_tools;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod terse_results;
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod wish_tools;
