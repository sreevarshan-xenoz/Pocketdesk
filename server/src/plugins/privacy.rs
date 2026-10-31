use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::{Value, json};

use crate::privacy;
use std::sync::Mutex;
use std::time::Duration;

/// The privacy packet: a request, or its answer (which says `ok`).
pub const PRIVACY: &str = "kdeconnect.pocketdesk.privacy";

/// Who holds the grab: the device, and for another of the owner's
/// computers (PC8, the window that shows this screen there) the connection
/// it asked on, so the grab ends as that connection does: never left on
/// with nobody there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Holder {
    device: String,
    connection: Option<String>,
}

/// Why a grab or release is refused while another device holds the grab
/// (PR #561 review M2: a computer may hold it now, or be the one refused).
pub(crate) const HELD_ELSEWHERE: &str = "another phone or computer holds privacy mode";

/// Why a grab is refused from the device this computer's shared keyboard
/// and mouse are on now (range review I3): privacy mode would swallow them
/// here, and that device could no longer be reached with them.
pub(crate) const DRIVEN_FROM_HERE: &str = "this computer's keyboard and mouse are on yours now (the shared keyboard and mouse); privacy mode would leave them nowhere";

/// Why a grab (a phone's too) is refused while this computer's keyboard and
/// mouse are away on any device (PR #572 review I1): privacy mode would
/// swallow the keys that bring them home.
pub(crate) const AWAY_ELSEWHERE: &str = "this computer's keyboard and mouse are on another device now (the shared keyboard and mouse); privacy mode would leave them nowhere";

/// Why a computer's grab is refused from one of this computer's
/// shared-input computers (PR #572 review I1): its keyboard and mouse may
/// be the only ones that reach the other, wherever they are this moment.
pub(crate) const SHARES_INPUT: &str = "this computer shares its keyboard and mouse with yours (the shared keyboard and mouse); privacy mode could leave you no way back";

/// How this computer's shared keyboard and mouse stand for a privacy ask
/// (PR #572 review I1): the device they are on now, and whether the asker
/// is one of this computer's shared-input computers, with shared input on.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Standing {
    pub driving: Option<String>,
    pub shares: bool,
}

/// [`Standing`] as this computer stands now, for `device`'s ask. Reads
/// files (blocking), and only for a computer's grab.
fn standing_now(action: &str, device: &str, computer: bool) -> Standing {
    let driving = crate::shared_input::status::keyboard_on_device();
    let shares = action == "grab" && computer && crate::shared_input::status::shares_with(device);
    Standing { driving, shares }
}

/// The device whose grab is held: only it may refresh or release it (R3).
static OWNER: Mutex<Option<Holder>> = Mutex::new(None);

/// Whether `device` may do `action` while `owner` holds the grab: anyone may
/// ask for the status or grab when nothing is held, only the holder may
/// refresh or release it.
fn may_act(action: &str, owner: Option<&str>, device: &str) -> bool {
    match (action, owner) {
        ("grab" | "release", Some(holder)) => holder == device,
        _ => true,
    }
}

/// Whether a renewal (PC8: `{action: "grab", renew: true}`, a computer's
/// window keeping its grab alive) is refused: only a grab still held by
/// `device` is renewed, so a grab that ended here (its connection closed,
/// its TTL ran out, released here) is not started again by a late renewal.
fn renewal_refused(renew: bool, owner: Option<&str>, device: &str) -> bool {
    renew && owner != Some(device)
}

/// Whether a packet's body is an answer (it says `ok`), never a request:
/// one computer's answer to another's window is never answered again.
pub(crate) fn is_answer(body: &Value) -> bool {
    body.get("ok").is_some()
}

/// `response` naming the request `body` it answers (PR #561 review M3):
/// the request's `id`, when it said one, so the asker matches the answer to
/// its ask.
pub(crate) fn answering(body: &Value, mut response: Value) -> Value {
    if let Some(id) = body.get("id") {
        response["id"] = id.clone();
    }
    response
}

/// The holder, forgotten once the grab itself has ended (released or its
/// TTL ran out).
fn holder() -> Option<Holder> {
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if owner.is_some() && privacy::privacy_status().0 == 0 {
        *owner = None;
    }
    owner.clone()
}

/// The device holding the grab.
fn owner() -> Option<String> {
    holder().map(|h| h.device)
}

/// Whether the grab held by `holder` ends as the connection
/// `connection_id` closes: only a computer's, on the connection it asked on.
fn ends_with(holder: &Holder, connection_id: &str) -> bool {
    holder.connection.as_deref() == Some(connection_id)
}

/// Give local input back if the phone holding the grab may no longer have
/// it (no longer paired): run by the live-session watcher each second.
pub(crate) fn release_if_owner(lost: impl Fn(&str) -> bool) {
    let Some(holder) = owner() else { return };
    if lost(&holder) {
        let count = privacy::release_input();
        *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        tracing::warn!(
            device_id = %holder,
            released = count,
            "Privacy grab released: the phone holding it is no longer paired"
        );
    }
}

/// Whether `device_id` holds the grab now, by the last word on it (no
/// look at the grab itself).
pub(crate) fn holds(device_id: &str) -> bool {
    OWNER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|h| h.device == device_id)
}

/// Whether the session here was unlocked since a computer's grab was seen
/// with it locked (PR #572 review I1): `was` is whether it was seen
/// locked, `held` whether a computer holds the grab, `locked` the session
/// as read now (`None`: not known).
fn unlocked_now(was: &mut bool, held: bool, locked: Option<bool>) -> bool {
    if !held {
        *was = false;
        return false;
    }
    match locked {
        Some(true) => {
            *was = true;
            false
        }
        Some(false) => std::mem::take(was),
        None => false,
    }
}

/// The holder in `owner` if it is a computer's (it asked on a connection or
/// a link), taken out; a phone's is left.
fn take_computer_holder(owner: &mut Option<Holder>) -> Option<Holder> {
    owner.take_if(|h| h.connection.is_some())
}

/// Whether the session was seen locked while a computer held the grab.
static SEEN_LOCKED: Mutex<bool> = Mutex::new(false);

/// The live-session watcher's look each second (PR #572 review I1): with a
/// computer holding the grab, the session here locked (Ctrl+Alt+Del) and
/// then unlocked lets the grab go, so whoever unlocks this computer always
/// gets its keyboard and mouse back. On Linux, logind's Unlock does the
/// same ([`session_unlocked`]). Blocking.
pub(crate) fn unlock_tick() {
    let held = OWNER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|h| h.connection.is_some());
    let locked = if held {
        privacy::session_locked()
    } else {
        None
    };
    let unlocked = unlocked_now(
        &mut SEEN_LOCKED.lock().unwrap_or_else(|e| e.into_inner()),
        held,
        locked,
    );
    if unlocked {
        session_unlocked();
    }
}

/// The session here was unlocked (PR #572 review I1): a grab a computer
/// holds is let go. Blocking.
pub(crate) fn session_unlocked() {
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(holder) = take_computer_holder(&mut owner) else {
        return;
    };
    let count = privacy::release_input();
    drop(owner);
    tracing::warn!(
        device_id = %holder.device,
        released = count,
        "Privacy grab released: this computer was unlocked here"
    );
}

/// A visit of this computer's shared keyboard and mouse to `device_id`
/// starts (range review I3): a grab it holds here is let go, so the keys
/// and clicks reach it. Best effort: while a grab is held, its hooks (or
/// `EVIOCGRAB`) keep the switch and the edges from shared input, so a
/// visit can rarely start; the refusals in [`refusal`] and the unlock
/// ([`session_unlocked`]) are what hold (PR #572 review I1). Blocking.
pub(crate) fn release_held_by(device_id: &str) {
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if take_held_by(&mut owner, device_id).is_none() {
        return;
    }
    let count = privacy::release_input();
    drop(owner);
    tracing::info!(
        device_id,
        released = count,
        "Privacy grab released: this computer's keyboard and mouse went to the device holding it"
    );
}

/// The connection `connection_id` closed (PC8): a computer's grab asked on
/// it ends with it. A phone's is kept (it may come back on a new
/// connection), to its TTL as before.
pub(crate) fn connection_closed(connection_id: &str) {
    // Releasing blocks (Windows joins its hook thread): off the async
    // worker when there is one (PR #561 re-review N5).
    let id = connection_id.to_string();
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(move || release_closed(&id));
        }
        Err(_) => release_closed(&id),
    }
}

/// [`connection_closed`]'s work, blocking.
fn release_closed(connection_id: &str) {
    // Checked, taken and released under one lock (PR #561 review M10): a
    // grab that another device wins in between is never left ownerless.
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(holder) = take_if_closed(&mut owner, connection_id) else {
        return;
    };
    let count = privacy::release_input();
    drop(owner);
    tracing::info!(
        device_id = %holder.device,
        released = count,
        "Privacy grab released: the computer holding it is no longer connected"
    );
}

/// The holder in `owner` whose grab ends as `connection_id` closes, taken
/// out; `owner` is left as it was for any other.
fn take_if_closed(owner: &mut Option<Holder>, connection_id: &str) -> Option<Holder> {
    owner.take_if(|h| ends_with(h, connection_id))
}

/// The holder in `owner` if it is `device_id`, taken out; `owner` is left
/// as it was for any other.
fn take_held_by(owner: &mut Option<Holder>, device_id: &str) -> Option<Holder> {
    owner.take_if(|h| h.device == device_id)
}

/// How long a computer's grab lives without a renewal (PR #561 review I1):
/// its window renews every 10 s, so three renewals missed end it, where a
/// phone's lives [`privacy::GRAB_TTL`] (10 minutes) as before.
pub const COMPUTER_GRAB_TTL: Duration = Duration::from_secs(30);

/// The grab's TTL for a holder that asked on `connection` (a computer) or
/// on none (a phone).
pub(crate) fn grab_ttl(connection: Option<&str>) -> Duration {
    if connection.is_some() {
        COMPUTER_GRAB_TTL
    } else {
        privacy::GRAB_TTL
    }
}

/// The connection id that the link this computer keeps to `device_id`
/// stands for (PC8), for a grab asked on that link.
pub(crate) fn link_connection(device_id: &str) -> String {
    format!("link:{device_id}")
}

/// Ends a grab asked on the link this computer keeps to a computer as the
/// link ends (PC8).
pub(crate) struct LinkGrab(pub String);

impl Drop for LinkGrab {
    fn drop(&mut self) {
        connection_closed(&link_connection(&self.0));
    }
}

/// Privacy mode (Tier-3 #15): the phone asks the desktop to block *local*
/// input while the remote session keeps working.
///
/// Answers `kdeconnect.pocketdesk.privacy`:
/// - `{action: "grab"}` — exclusively grab all physical keyboards/mice
///   (`EVIOCGRAB`); the compositor stops seeing local input, our uinput
///   injection path is untouched. Re-arms the auto-release TTL on refresh;
///   with `renew: true` (PC8) only a grab the asker still holds.
/// - `{action: "release"}` — give local input back.
/// - `{action: "status"}` — report held devices + seconds of TTL left.
/// - `{lock: true}` (combinable with grab) — also engage the screen locker.
///
/// Every reply is `{ok, grabbed?, ttlLeft?, locked?, error?}`. Another of
/// the owner's computers asks the same (PC8, its window's "Hide what I do
/// there"), held to the same Power switch; its grab also ends as the
/// connection it asked on closes.
#[derive(Debug, Default)]
pub struct PrivacyPlugin;

/// Answer a privacy request (a grab, a release, a lock) from a phone with
/// Power turned off, which the control gate drops: the phone waits for a
/// reply, and is told why at once instead of after its timeout (U2). An
/// answer is never answered (PC8).
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type != PRIVACY || is_answer(&packet.body) {
        return;
    }
    let computer = computer(sender.device_id()).await;
    let body = answering(
        &packet.body,
        json!({ "ok": false, "error": denied_words(computer) }),
    );
    let _ = sender
        .send_packet(&NetworkPacket::new(PRIVACY).with_body(body))
        .await;
}

/// Why a request from a device without Power is refused: a computer is
/// told the switch by its name on this computer's Devices page (PC8).
pub(crate) fn denied_words(computer: bool) -> &'static str {
    if computer {
        "\u{201c}Lock, sleep, privacy and wake others\u{201d} is off for this computer there"
    } else {
        "Power is turned off for this phone on the computer"
    }
}

/// Whether `device_id` is another of the owner's computers (the trust
/// store, read off the runtime).
async fn computer(device_id: &str) -> bool {
    let id = device_id.to_string();
    tokio::task::spawn_blocking(move || crate::plugins::pair::is_computer_device(&id))
        .await
        .unwrap_or(false)
}

/// Do what the privacy request `body` from `device` asks and say how it
/// went (`{ok, grabbed?, ttlLeft?, locked?, error?}`). `connection` is the
/// connection a computer asked on, its grab ending with it (PC8); `None`
/// for a phone. `ttl` is how long a grab lives without a refresh
/// ([`grab_ttl`]). Shared by the plugin and the link this computer keeps to
/// a computer.
pub(crate) async fn respond(
    body: &Value,
    device: &str,
    connection: Option<&str>,
    ttl: Duration,
) -> Value {
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    let want_lock = body.get("lock").and_then(|v| v.as_bool()).unwrap_or(false);
    let renew = body.get("renew").and_then(|v| v.as_bool()).unwrap_or(false);

    let (device_owned, connection_owned) = (device.to_string(), connection.map(str::to_string));
    let action_owned = action.to_string();
    // Checked and done under one lock (PR #561 re-review N4), off the
    // runtime: grabbing and releasing block.
    let (mut response, refused) = tokio::task::spawn_blocking(move || {
        act(
            &action_owned,
            renew,
            &device_owned,
            connection_owned.as_deref(),
            ttl,
        )
    })
    .await
    .unwrap_or_else(|e| (json!({ "ok": false, "error": e.to_string() }), true));

    // A refused request does nothing, the lock included.
    if want_lock && !refused {
        // The lock helpers use blocking process spawns.
        let outcome = tokio::task::spawn_blocking(privacy::lock_screen).await;
        match outcome {
            Ok(Ok(program)) => response["locked"] = json!(program),
            Ok(Err(e)) => response["lockError"] = json!(e),
            Err(e) => response["lockError"] = json!(e.to_string()),
        }
    }
    answering(body, response)
}

/// Why `device` may not do `action` (with `renew`) while `holder` holds the
/// grab and this computer's keyboard and mouse are on `driving` (the
/// shared keyboard and mouse), as the answer says it, and whether a lock
/// asked with it is refused too; `None` when it may.
fn refusal(
    action: &str,
    renew: bool,
    holder: Option<&str>,
    device: &str,
    computer: bool,
    standing: &Standing,
) -> Option<(Value, bool)> {
    if action == "grab" {
        // The device these keys and clicks are on would lose them (range
        // review I3); a renewal as much as a first grab.
        let refused = if standing.driving.as_deref() == Some(device) {
            Some(DRIVEN_FROM_HERE)
        } else if standing.driving.is_some() {
            // Away elsewhere: the keys that bring them home would be
            // swallowed (PR #572 review I1).
            Some(AWAY_ELSEWHERE)
        } else if computer && standing.shares {
            // Judged on standing, not on where the keyboard is this moment
            // (PR #572 review I1): it may come home before the ask lands.
            Some(SHARES_INPUT)
        } else {
            None
        };
        if let Some(why) = refused {
            return Some((json!({ "ok": false, "error": why }), true));
        }
    }
    if !may_act(action, holder, device) {
        return Some((json!({ "ok": false, "error": HELD_ELSEWHERE }), true));
    }
    if action == "grab" && renewal_refused(renew, holder, device) {
        return Some((
            json!({ "ok": false, "error": "privacy mode ended there" }),
            false,
        ));
    }
    None
}

/// Do `action` for `device`, the holder checked and the grab taken or let
/// go under one lock, so no other request or closing connection comes
/// between (PR #561 re-review N4). Returns the answer and whether the
/// request was refused (a lock with it is then not done).
fn act(
    action: &str,
    renew: bool,
    device: &str,
    connection: Option<&str>,
    ttl: Duration,
) -> (Value, bool) {
    act_standing(action, renew, device, connection, ttl, standing_now)
}

/// [`act`], with how this computer's shared keyboard and mouse stand read
/// by `standing` (the action, the asker, whether it is a computer).
fn act_standing(
    action: &str,
    renew: bool,
    device: &str,
    connection: Option<&str>,
    ttl: Duration,
    standing: impl FnOnce(&str, &str, bool) -> Standing,
) -> (Value, bool) {
    // A computer asks on a connection or a link of its own (PC8).
    let computer = connection.is_some();
    let standing = standing(action, device, computer);
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if owner.is_some() && privacy::privacy_status().0 == 0 {
        *owner = None;
    }
    let holder = owner.as_ref().map(|h| h.device.clone());
    if let Some((answer, refused)) = refusal(
        action,
        renew,
        holder.as_deref(),
        device,
        computer,
        &standing,
    ) {
        if refused {
            tracing::warn!(
                device_id = %device,
                holder = holder.as_deref().unwrap_or(""),
                "Privacy {action} refused: another device holds the grab, or this keyboard and mouse are on the asker"
            );
        }
        return (answer, refused);
    }
    let answer = match action {
        "grab" => match privacy::grab_input(ttl) {
            Ok(count) => {
                *owner = Some(Holder {
                    device: device.to_string(),
                    connection: connection.map(str::to_string),
                });
                json!({ "ok": true, "grabbed": count })
            }
            Err(e) => json!({ "ok": false, "error": e }),
        },
        "release" => {
            let count = privacy::release_input();
            *owner = None;
            json!({ "ok": true, "released": count })
        }
        _ => {
            let (count, ttl_left) = privacy::privacy_status();
            json!({ "ok": true, "grabbed": count, "ttlLeft": ttl_left })
        }
    };
    (answer, false)
}

#[async_trait::async_trait]
impl Plugin for PrivacyPlugin {
    fn name(&self) -> &'static str {
        "privacy"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PRIVACY]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[PRIVACY]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        // An answer to this computer's window (PC8) is taken before the
        // gate (`take_answer`); one that comes here anyway is never
        // answered.
        if packet.packet_type != PRIVACY || is_answer(&packet.body) {
            return Ok(());
        }
        let device = sender.device_id();
        // A computer's grab ends with the connection it asked on (PC8).
        let connection = computer(device)
            .await
            .then(|| sender.connection_id().to_string());
        let ttl = grab_ttl(connection.as_deref());
        let response = respond(&packet.body, device, connection.as_deref(), ttl).await;
        sender
            .send_packet(&NetworkPacket::new(PRIVACY).with_body(response))
            .await
    }
}

/// Take the answer to a privacy ask this computer's window made of the
/// computer `device_id` (PC8), before the permission gate: an answer asks
/// nothing of this computer, so this computer's own switches for that one
/// do not decide whether it is heard. Whether it is used is for the view
/// that asked ([`crate::computer_view::privacy_answer`]). Returns whether
/// `packet` was such an answer.
pub(crate) fn take_answer(device_id: &str, packet: &NetworkPacket) -> bool {
    if packet.packet_type != PRIVACY || !is_answer(&packet.body) {
        return false;
    }
    crate::computer_view::privacy_answer(device_id, &packet.body);
    true
}

/// Tests elsewhere: make `device` the holder, asked on `connection`, as a
/// grab would, without grabbing anything.
#[cfg(test)]
pub(crate) fn hold_for_test(device: &str, connection: &str) {
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Holder {
        device: device.to_string(),
        connection: Some(connection.to_string()),
    });
}

/// Tests elsewhere: whether `device` is the holder.
#[cfg(test)]
pub(crate) fn held_by_test(device: &str) -> bool {
    OWNER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|h| h.device == device)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_phone_that_grabbed_may_refresh_or_release() {
        assert!(may_act("grab", None, "phone-a"), "nothing held");
        assert!(
            may_act("grab", Some("phone-a"), "phone-a"),
            "its own refresh"
        );
        assert!(!may_act("grab", Some("phone-a"), "phone-b"));
        assert!(may_act("release", Some("phone-a"), "phone-a"));
        assert!(!may_act("release", Some("phone-a"), "phone-b"));
        assert!(may_act("release", None, "phone-b"), "nothing to protect");
        assert!(
            may_act("status", Some("phone-a"), "phone-b"),
            "anyone may ask"
        );
    }

    /// PC8: a computer's window renews only a grab it still holds here.
    #[test]
    fn a_renewal_does_not_start_a_grab_that_ended() {
        assert!(!renewal_refused(true, Some("desk"), "desk"), "still held");
        assert!(renewal_refused(true, None, "desk"), "ended here");
        assert!(renewal_refused(true, Some("phone-a"), "desk"));
        assert!(!renewal_refused(false, None, "desk"), "a first grab");
    }

    /// PC8: a computer's grab ends with the connection it asked on; a
    /// phone's waits for its TTL as before.
    #[test]
    fn a_computers_grab_ends_with_its_connection() {
        let computer = Holder {
            device: "desk".into(),
            connection: Some("conn-7".into()),
        };
        assert!(ends_with(&computer, "conn-7"));
        assert!(!ends_with(&computer, "conn-8"), "another connection of it");
        let phone = Holder {
            device: "phone-a".into(),
            connection: None,
        };
        assert!(!ends_with(&phone, "conn-7"));
        assert_eq!(link_connection("desk"), "link:desk");
    }

    /// PR #561 review I1: a computer's grab lapses after three missed
    /// renewals of its window, a phone's after 10 minutes as before.
    #[test]
    fn a_computers_grab_lapses_soon_without_its_renewals() {
        assert_eq!(grab_ttl(Some("conn-7")), COMPUTER_GRAB_TTL);
        assert_eq!(
            COMPUTER_GRAB_TTL,
            3 * crate::computer_view::PRIVACY_RENEW,
            "three renewals"
        );
        assert_eq!(grab_ttl(None), privacy::GRAB_TTL);
        assert!(COMPUTER_GRAB_TTL < privacy::GRAB_TTL);
    }

    /// PR #561 review M2: a refusal for a grab held elsewhere names both
    /// kinds of holder.
    #[test]
    fn a_grab_held_elsewhere_is_said_for_a_phone_or_a_computer() {
        let (said, refused) = refusal(
            "grab",
            false,
            Some("phone-a"),
            "desk",
            false,
            &Standing::default(),
        )
        .unwrap();
        assert!(refused, "a lock with it is not done");
        assert_eq!(
            said["error"], "another phone or computer holds privacy mode",
            "{said}"
        );
        let (said, refused) = refusal(
            "release",
            false,
            Some("desk"),
            "phone-a",
            false,
            &Standing::default(),
        )
        .unwrap();
        assert!(refused);
        assert_eq!(said["error"], HELD_ELSEWHERE);
        let (said, refused) =
            refusal("grab", true, None, "desk", false, &Standing::default()).unwrap();
        assert!(!refused, "a renewal refused does not refuse a lock");
        assert_eq!(said["error"], "privacy mode ended there");
        assert_eq!(
            refusal(
                "status",
                false,
                Some("phone-a"),
                "desk",
                false,
                &Standing::default()
            ),
            None
        );
        assert_eq!(
            refusal("grab", false, None, "desk", false, &Standing::default()),
            None
        );
    }

    /// Range review I3, PR #572 review I1: a grab (and a lock with it) is
    /// refused from the device these keys are on, from any computer while
    /// they are away, and from a shared-input computer wherever they are; a
    /// renewal too. Its release and status still go; a phone is judged as
    /// before.
    #[test]
    fn a_grab_is_refused_on_the_shared_keyboards_standing() {
        let on = |device: &str| Standing {
            driving: Some(device.to_string()),
            shares: false,
        };
        let shares = Standing {
            driving: None,
            shares: true,
        };
        let (said, refused) = refusal("grab", false, None, "desk", true, &on("desk")).unwrap();
        assert!(refused, "a lock with it is not done");
        assert_eq!(said["error"], DRIVEN_FROM_HERE);
        let (said, _) = refusal("grab", false, None, "desk", true, &on("laptop")).unwrap();
        assert_eq!(said["error"], AWAY_ELSEWHERE);
        let (said, _) = refusal("grab", false, None, "desk", true, &shares).unwrap();
        assert_eq!(said["error"], SHARES_INPUT);
        let (said, refused) = refusal("grab", true, Some("desk"), "desk", true, &shares).unwrap();
        assert!(refused, "nor renewed");
        assert_eq!(said["error"], SHARES_INPUT);
        assert_eq!(
            refusal("release", false, Some("desk"), "desk", true, &shares),
            None
        );
        assert_eq!(
            refusal("status", false, None, "desk", true, &on("desk")),
            None
        );
        let (said, _) = refusal("grab", false, None, "phone", false, &on("laptop")).unwrap();
        assert_eq!(
            said["error"], AWAY_ELSEWHERE,
            "a phone too, the keys elsewhere (PR #572 re-review N8)"
        );
        assert_eq!(
            refusal("grab", false, None, "phone", false, &Standing::default()),
            None,
            "a phone, the keys home"
        );
        assert_eq!(
            refusal("grab", false, None, "desk", true, &Standing::default()),
            None
        );
    }

    /// PR #572 review I1, through `act`: a computer's grab and renewal from
    /// a shared-input computer are refused before anything is grabbed, and
    /// its standing is asked with the asker and that it is a computer.
    #[test]
    fn act_refuses_a_shared_input_computer_before_grabbing() {
        let mut asked = None;
        let (said, refused) = act_standing(
            "grab",
            false,
            "pr572-desk",
            Some("link:pr572-desk"),
            COMPUTER_GRAB_TTL,
            |action, device, computer| {
                asked = Some((action.to_string(), device.to_string(), computer));
                Standing {
                    driving: None,
                    shares: true,
                }
            },
        );
        assert!(refused);
        assert_eq!(said["error"], SHARES_INPUT);
        assert_eq!(asked, Some(("grab".into(), "pr572-desk".into(), true)));
        let (said, refused) = act_standing(
            "grab",
            true,
            "pr572-desk",
            Some("link:pr572-desk"),
            COMPUTER_GRAB_TTL,
            |_, _, _| Standing {
                driving: Some("pr572-elsewhere".into()),
                shares: false,
            },
        );
        assert!(refused, "the renewal");
        assert_eq!(said["error"], AWAY_ELSEWHERE);
        assert!(!held_by_test("pr572-desk"), "nothing held");
    }

    /// PR #572 review I1: a computer's grab ends once the session here was
    /// seen locked and then unlocked; not while locked, not with nothing
    /// known, and a phone's grab is left.
    #[test]
    fn an_unlock_here_ends_a_computers_grab() {
        let mut was = false;
        assert!(
            !unlocked_now(&mut was, true, Some(false)),
            "never seen locked"
        );
        assert!(!unlocked_now(&mut was, true, Some(true)));
        assert!(!unlocked_now(&mut was, true, None), "not known");
        assert!(unlocked_now(&mut was, true, Some(false)), "unlocked");
        assert!(!unlocked_now(&mut was, true, Some(false)), "once");
        assert!(!unlocked_now(&mut was, true, Some(true)));
        assert!(!unlocked_now(&mut was, false, Some(false)), "no grab held");
        assert!(!was, "forgotten with the grab");

        let computer = Holder {
            device: "desk".into(),
            connection: Some("link:desk".into()),
        };
        let mut owner = Some(computer.clone());
        assert_eq!(take_computer_holder(&mut owner), Some(computer));
        let phone = Holder {
            device: "phone".into(),
            connection: None,
        };
        let mut owner = Some(phone.clone());
        assert_eq!(take_computer_holder(&mut owner), None);
        assert_eq!(owner, Some(phone), "a phone's grab stays");
    }

    /// Range review I3: a visit to the device holding the grab takes it out;
    /// a visit elsewhere leaves it.
    #[test]
    fn a_visit_takes_out_only_the_grab_of_the_device_visited() {
        let held = Holder {
            device: "desk".into(),
            connection: Some("link:desk".into()),
        };
        let mut owner = Some(held.clone());
        assert_eq!(take_held_by(&mut owner, "laptop"), None);
        assert_eq!(owner, Some(held.clone()));
        assert_eq!(take_held_by(&mut owner, "desk"), Some(held));
        assert_eq!(owner, None);
    }

    /// PR #561 review M10: a closing connection takes out only the holder
    /// that asked on it, in one step.
    #[test]
    fn a_closing_connection_takes_out_only_its_own_holder() {
        let computer = Holder {
            device: "desk".into(),
            connection: Some("conn-7".into()),
        };
        let mut owner = Some(computer.clone());
        assert_eq!(take_if_closed(&mut owner, "conn-8"), None);
        assert_eq!(
            owner,
            Some(computer.clone()),
            "another connection leaves it"
        );
        assert_eq!(take_if_closed(&mut owner, "conn-7"), Some(computer));
        assert_eq!(owner, None);
        let phone = Holder {
            device: "phone-a".into(),
            connection: None,
        };
        let mut owner = Some(phone.clone());
        assert_eq!(take_if_closed(&mut owner, "phone-a"), None);
        assert_eq!(owner, Some(phone));
    }

    /// PC8: an answer is never taken for a request, so two computers never
    /// answer each other's answers.
    #[test]
    fn an_answer_is_told_from_a_request() {
        assert!(is_answer(&json!({"ok": false, "error": "no"})));
        assert!(is_answer(&json!({"ok": true, "grabbed": 2})));
        assert!(!is_answer(&json!({"action": "grab"})));
        assert!(!is_answer(&json!({"action": "release", "lock": true})));
        assert!(!is_answer(&json!({})));
        assert!(denied_words(true).contains("this computer"));
        assert!(
            denied_words(false).contains("Power"),
            "a phone's words kept"
        );
    }

    /// PR #561 review M3: an answer names the request it answers.
    #[test]
    fn an_answer_names_its_request() {
        assert_eq!(
            answering(&json!({"action": "grab", "id": 4}), json!({"ok": true})),
            json!({"ok": true, "id": 4})
        );
        assert_eq!(
            answering(&json!({"action": "grab"}), json!({"ok": true})),
            json!({"ok": true}),
            "a phone's request names none"
        );
    }

    #[test]
    fn plugin_declares_privacy_capability() {
        assert!(PrivacyPlugin.incoming_capabilities().contains(&PRIVACY));
    }

    struct Phone(std::sync::Mutex<Vec<NetworkPacket>>);

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone-a"
        }
        fn connection_id(&self) -> &str {
            "phone-a"
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.0.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// A phone with Power off asking to lock is told so at once (U2): it
    /// had waited 5 s for a reply that never came, then said the computer
    /// did not answer.
    #[tokio::test]
    async fn a_refused_privacy_request_is_answered_with_why() {
        // The answer asks the trust store whether it is a computer.
        let _state = crate::state::use_test_dirs();
        let phone = Phone(Default::default());
        let asked =
            NetworkPacket::new(PRIVACY).with_body(json!({"action": "status", "lock": true}));
        crate::plugins::answer_denied(&asked, &phone).await;
        let sent = phone.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, PRIVACY);
        assert_eq!(sent[0].body["ok"], false);
        assert!(sent[0].body["error"].as_str().unwrap().contains("Power"));
        assert!(sent[0].body.get("locked").is_none());
    }

    /// An answer that reaches the gate is never answered (PC8).
    #[tokio::test]
    async fn an_answer_refused_by_the_gate_is_not_answered() {
        let phone = Phone(Default::default());
        let answer = NetworkPacket::new(PRIVACY).with_body(json!({"ok": true, "grabbed": 2}));
        crate::plugins::answer_denied(&answer, &phone).await;
        assert!(phone.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn status_action_replies_without_grabbing() {
        let (count, ttl) = privacy::privacy_status();
        assert_eq!(count, 0, "test run must not hold real grabs");
        assert!(ttl.is_none());
        let packet = NetworkPacket::new(PRIVACY).with_body(json!({ "action": "status" }));
        assert_eq!(packet.packet_type, PRIVACY);
    }
}
