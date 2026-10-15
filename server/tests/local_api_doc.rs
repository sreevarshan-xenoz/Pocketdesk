//! `docs/local-api.md` is the local API's contract: every command that takes
//! `--json` must be described there.

use clap::CommandFactory;
use pocketdesk_server::cli::Cli;

#[test]
fn every_command_with_json_is_in_the_local_api_doc() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/local-api.md"))
        .expect("docs/local-api.md");
    let missing: Vec<String> = Cli::command()
        .get_subcommands()
        .filter(|sub| sub.get_arguments().any(|arg| arg.get_id() == "json"))
        .map(|sub| sub.get_name().to_string())
        // The whole name, so `list` is not found inside `listen`.
        .filter(|name| {
            ![" ", "`", " --"]
                .iter()
                .any(|end| doc.contains(&format!("`pocketdesk {name}{end}")))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "commands with --json missing from docs/local-api.md: {missing:?}"
    );
}
