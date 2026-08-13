//! Assertions about the MCP tool surface.
//!
//! These live outside the library because building the tool router links the
//! `commands` module, and with it `rfd`'s Windows task dialog — which needs a
//! Common-Controls v6 manifest that only `[[test]]` targets can be given. See
//! `build.rs`. Keeping them here also leaves the library's own unit-test binary
//! free of that dependency.

use app_lib::mcp::server::BlogMcp;

/// The exposed tool set, asserted exactly.
///
/// Two failures this catches. First, a mis-wired `#[tool_handler]` registers
/// nothing and the server answers `tools/list` with an empty list — it still
/// compiles and still starts, so nothing else would notice. Second, and the
/// reason the list is exact rather than a subset: a tool that publishes without
/// going through the approval queue would be a way around the human gate, so a
/// new name showing up here has to be a deliberate edit.
#[test]
fn the_tool_surface_is_exactly_what_we_intend() {
    let mut names: Vec<String> = BlogMcp::tool_router()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();

    assert_eq!(
        names,
        [
            "create_draft",
            "get_post",
            "list_media",
            "list_posts",
            "list_series",
            "publish_status",
            "request_publish",
            "update_draft",
        ]
    );
}

/// Descriptions are the only thing telling an agent that `request_publish` does
/// not actually publish, so an undescribed tool is a real defect.
#[test]
fn every_tool_carries_a_description() {
    for tool in BlogMcp::tool_router().list_all() {
        let described = tool.description.as_ref().is_some_and(|d| !d.trim().is_empty());
        assert!(described, "tool `{}` has no description", tool.name);
    }
}

/// The gate in one assertion: the only tool that mentions publishing must say it
/// needs approval, and no tool may offer to publish outright.
#[test]
fn the_publish_tool_advertises_the_approval_gate() {
    let router = BlogMcp::tool_router();
    let tools = router.list_all();

    let request = tools
        .iter()
        .find(|t| t.name == "request_publish")
        .expect("request_publish must exist");
    let description = request.description.as_deref().unwrap_or_default().to_lowercase();

    assert!(
        description.contains("approve") || description.contains("approval"),
        "request_publish must tell the agent a human has to approve: {description}"
    );
    assert!(
        description.contains("does not publish") || description.contains("not publish"),
        "request_publish must be explicit that it does not itself publish: {description}"
    );
}
