//! `docs/capabilities.md` is generated, so it must be what *this* build reports.
//!
//! Roadmap Phase 0 item 4 makes `linux-link capabilities` the single source of
//! truth for negotiated protocol versions, transports, capture backends and
//! codecs. A generated file is only a source of truth if drift is caught: this
//! test renders the report from the same code path the CLI uses and compares it
//! to the committed doc, so a changed constant or a re-ordered capture ladder
//! fails CI until the doc is regenerated.

use linux_link_core::capabilities::Capabilities;

const DOC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/capabilities.md");

#[test]
fn capabilities_doc_matches_the_build_that_reports_it() {
    let expected = Capabilities::collect().render_markdown();
    let actual = std::fs::read_to_string(DOC)
        .unwrap_or_else(|e| panic!("cannot read {DOC}: {e} — regenerate it"))
        // A Windows checkout made with core.autocrlf=true has CRLF here even
        // though the committed file (and the renderer) are LF.
        .replace("
", "
");
    assert_eq!(
        expected.trim_end(),
        actual.trim_end(),
        "docs/capabilities.md is stale. Regenerate with:\n    \
         cargo run -q -p linux-link-server -- capabilities --markdown > docs/capabilities.md"
    );
}
