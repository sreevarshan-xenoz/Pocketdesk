//! `docs/capabilities.md` is generated, so it must be what *this* build reports.
//!
//! Roadmap Phase 0 item 4 makes `linux-link capabilities` the single source of
//! truth for negotiated protocol versions, transports, capture backends and
//! codecs. A generated file is only a source of truth if drift is caught: this
//! test renders the report from the same code path the CLI uses and compares it
//! to the committed doc, so a changed constant or a re-ordered capture ladder
//! fails CI until the doc is regenerated.

use linux_link_core::capabilities::Capabilities;

/// Each OS's build reports what *it* can do, so each has its own committed doc.
#[cfg(not(windows))]
const DOC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/capabilities.md");
#[cfg(windows)]
const DOC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/capabilities-windows.md"
);

#[test]
fn capabilities_doc_matches_the_build_that_reports_it() {
    let expected = Capabilities::collect().render_markdown();
    let actual = std::fs::read_to_string(DOC)
        .unwrap_or_else(|e| panic!("cannot read {DOC}: {e} — regenerate it"))
        // A Windows checkout made with core.autocrlf=true has CRLF here even
        // though the committed file (and the renderer) are LF.
        .replace("\r\n", "\n");
    assert_eq!(
        expected.trim_end(),
        actual.trim_end(),
        "{DOC} is stale. Regenerate it on the OS it describes (capabilities.md on Linux, \
         capabilities-windows.md on Windows) with:\n    \
         cargo run -q -p linux-link-server -- capabilities --markdown > <that file>"
    );
}
