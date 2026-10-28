//! The desktop app's requests for the shared keyboard and mouse, one file
//! each in `shared_input_out/`: Identify, which has each computer this one
//! can drive now flash its own name ([`Msg::Identify`], carrying this
//! computer's name, the one that asks, for the other's log), and this one
//! its own.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use pocketdesk_core::local::paths::SHARED_INPUT_DIR;
use pocketdesk_core::local::shared_input::{self, ShareAsk};

use super::badge;
use super::link::{self, Channel};
use super::wire::Msg;

/// How often the folder is looked at when it cannot be watched.
const POLL: Duration = Duration::from_millis(500);

/// The computers an Identify goes to: linked directly now, that answered
/// with their screens and did not refuse since (`direct`, whether each
/// linked one is direct).
pub fn identify_targets(
    direct: &HashMap<String, bool>,
    answered: &BTreeSet<String>,
    refused: &BTreeSet<String>,
) -> Vec<String> {
    let mut to: Vec<String> = direct
        .iter()
        .filter(|(id, direct)| **direct && answered.contains(*id) && !refused.contains(*id))
        .map(|(id, _)| id.clone())
        .collect();
    to.sort();
    to
}

/// Ask each computer this one can drive now to show who it is, and show
/// this one's name here too.
async fn identify() {
    let name = tokio::task::spawn_blocking(|| crate::kde::host_identity().device_name)
        .await
        .unwrap_or_default();
    crate::overlay::flash(
        &badge::text(&badge::Event::Identify { here: &name }),
        badge::IDENTIFY_FOR,
    );
    let direct: HashMap<String, bool> = link::linked()
        .into_iter()
        .filter_map(|id| {
            let direct = link::is_direct(&link::connection(&id)?);
            Some((id, direct))
        })
        .collect();
    let answered = link::screens_heard().into_keys().collect();
    let refused = link::refusals().into_keys().collect();
    let to = identify_targets(&direct, &answered, &refused);
    tracing::info!(
        computers = to.len(),
        "Shared input: Identify: {name} asks the computers it can drive to show who they are"
    );
    for id in to {
        // A channel of its own, dropped once sent: its stream is finished
        // after the message.
        let sent = Channel::open(&id)
            .and_then(|mut channel| channel.send(&Msg::Identify { name: name.clone() }));
        if let Err(e) = sent {
            tracing::debug!(to = id, "Shared input: Identify not sent: {e}");
        }
    }
}

/// Take the app's requests, for the server's life.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        let taken = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || {
                shared_input::take_all(&dir, pocketdesk_core::local::pin::now_unix())
            })
            .await
            .unwrap_or_default()
        };
        for request in taken {
            match request.ask {
                ShareAsk::Identify => identify().await,
            }
        }
        crate::inbox::wait(SHARED_INPUT_DIR, POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_goes_to_the_computers_this_one_can_drive() {
        let direct = HashMap::from([
            ("A".to_string(), true),
            ("relayed".to_string(), false),
            ("quiet".to_string(), true),
            ("refusing".to_string(), true),
        ]);
        let answered = BTreeSet::from([
            "A".to_string(),
            "relayed".to_string(),
            "refusing".to_string(),
            "unlinked".to_string(),
        ]);
        let refused = BTreeSet::from(["refusing".to_string()]);
        assert_eq!(identify_targets(&direct, &answered, &refused), ["A"]);
    }
}
