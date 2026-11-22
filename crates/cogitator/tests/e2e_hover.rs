//! End-to-end suite (specs/rust-port/05 §7) — this file's theme only;
//! the framed-LSP harness lives in tests/harness/.

mod harness;
use harness::jaffle;

/// Hover never computes lineage on the dispatcher, but a miss fills the
/// cache off-thread, so a later hover of the same text has the line.
#[test]
fn a_hover_miss_warms_lineage_for_the_next_hover() {
    let project = jaffle();
    let (mut lsp, _uri) = project.booted();
    let probe = cogitator_lsp::fs_path_to_uri(&project.root.join("models/hover_probe.sql"));
    lsp.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": probe, "languageId": "sql", "version": 1,
            "text": "select order_id from {{ ref('orders') }}\n"}}),
    );

    let contents = lsp.wait_until("hover shows the column's lineage", |lsp| {
        let hover = lsp.request(
            "textDocument/hover",
            serde_json::json!({"textDocument": {"uri": probe},
                               "position": {"line": 0, "character": 9}}),
        );
        let text = hover.to_string();
        text.contains("orders.order_id").then_some(text)
    });
    assert!(contents.contains("Column of orders"), "{contents}");
}
