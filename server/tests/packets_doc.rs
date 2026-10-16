//! `docs/packets.md` is generated (ecosystem 1g), so it must be the catalogue
//! *this* build's plugins make: a new or renamed packet type, or a changed
//! permission, fails CI until the doc is regenerated with
//! `pocketdesk capabilities --packets > docs/packets.md`.

const DOC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/packets.md");

#[test]
fn the_packet_catalogue_matches_the_build() {
    let expected = pocketdesk_server::packets::render_markdown();
    let actual = std::fs::read_to_string(DOC)
        .unwrap_or_else(|e| panic!("cannot read {DOC}: {e} — regenerate it"))
        .replace("\r\n", "\n");
    assert_eq!(
        expected.trim_end(),
        actual.trim_end(),
        "docs/packets.md is stale: run `pocketdesk capabilities --packets > docs/packets.md`"
    );
}
