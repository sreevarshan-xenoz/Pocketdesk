//! A window lent from KDE Plasma, captured through the screen-sharing portal
//! (window handoff, phase 6 follow-up).
//!
//! KDE's own window capture (`zkde_screencast_unstable_v1`) is not used: it
//! is a compositor interface that needs a client binding of KDE's protocol
//! (not ours to copy) and a desktop-file entry granting this program a
//! privileged interface. The portal asks the owner instead, which is the
//! price of a grant: **one dialog for each window, the first time**, and with
//! the grant kept (the portal's restore token, one file for each window in
//! the grants folder, owner-only) **none after that**.
//!
//! - The portal is asked for window sources only, one at a time, with the
//!   pointer hidden, and to keep the grant until the owner revokes it
//!   ([`PersistMode::ExplicitlyRevoked`]); the token kept for the lent
//!   window, if there is one, is offered.
//! - Whoever answers the dialog may pick any window, and a restored grant
//!   brings back whatever it was made for, so the stream the portal answers
//!   with is matched to the lent window ([`window_kde::match_stream`]) against
//!   the table KWin's script reports, read again after the dialog: one stream,
//!   of a window, named as the lent window or of its size with no look-alike.
//!   If it cannot be shown to be the lent window, nothing is captured, the
//!   token is not kept (and one that was is forgotten), and the session ends
//!   saying so.
//! - **No input.** The session is a ScreenCast session only: no RemoteDesktop
//!   link, nothing in it can send input (`window_target::takes_input` is also
//!   false for the lease, so the session is view-only before it starts).
//! - The session is closed, and the streaming session ended, when the lease
//!   stops standing (the window was given back, closed, swept or lent again),
//!   as for GNOME's capture, and when the streaming session ends.
//!
//! Nothing here falls back to a monitor.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::capture::CaptureSession;
use super::window_kde::{self, KdeWindow, PortalStream, Refusal, StreamKind};
use super::window_target::{self, Lease};
use super::{StreamingConfig, VideoFrame};

/// How long the portal's Start may take: the first time for a window, its
/// owner reads and answers the dialog.
const START_WAIT: Duration = Duration::from_secs(60);

/// How long any other portal call may take.
const CALL_WAIT: Duration = Duration::from_secs(10);

/// How old the table may be for the match of a stream to the window: a
/// window's size and identity change only with a report from KWin's own
/// signals, so a quiet table is still right; the script is asked to report
/// first, and a table with no report for this long is not believed.
const MATCH_MAX_AGE: Duration = Duration::from_secs(90);

/// The lent window, from the table after the script has been asked to report
/// (for up to two seconds), if the table is not older than [`MATCH_MAX_AGE`].
async fn lent_window(seq: u64) -> Result<(KdeWindow, std::sync::Arc<window_kde::Snapshot>)> {
    let mut updates = window_kde::updates();
    updates.mark_unchanged();
    if !window_kde::snapshot().is_some_and(|t| t.is_fresh(Instant::now())) {
        window_kde::poke();
        let _ = tokio::time::timeout(Duration::from_secs(2), updates.changed()).await;
    }
    let table = window_kde::snapshot()
        .filter(|t| t.at.elapsed() < MATCH_MAX_AGE)
        .ok_or_else(|| anyhow::anyhow!(window_kde::say(Refusal::TableStale)))?;
    let lent = table
        .window(seq)
        .cloned()
        .context("The lent window is not on this computer's desktop any more")?;
    Ok((lent, table))
}

/// What the portal answered with, reduced to what shows whose it is.
fn portal_streams(streams: &[ashpd::desktop::screencast::Stream]) -> Vec<PortalStream> {
    streams
        .iter()
        .map(|s| PortalStream {
            node: s.pipe_wire_node_id(),
            kind: match s.source_type() {
                Some(SourceType::Window) => StreamKind::Window,
                Some(SourceType::Monitor) => StreamKind::Monitor,
                Some(SourceType::Virtual) => StreamKind::Virtual,
                None => StreamKind::Unknown,
            },
            size: s.size(),
            names: [s.id(), s.mapping_id()]
                .into_iter()
                .flatten()
                .map(str::to_string)
                .collect(),
        })
        .collect()
}

/// Start the capture of the window `lease` is held on: see the module's
/// notes. Failing to start leaves `cancel` alone; once started, it ends, and
/// cancels `cancel` (which ends the streaming session), when the lease does
/// not stand any more.
pub async fn start_window_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    lease: Lease,
) -> Result<CaptureSession> {
    let seq = window_kde::kde_seq(&lease.native).context("that lease is not for a KDE window")?;
    if !window_target::lease_alive(&lease) {
        anyhow::bail!("That window is not lent to this session any more");
    }
    let (lent, _) = lent_window(seq).await?;
    let unavailable = || anyhow::anyhow!(window_kde::say(Refusal::NoPortal));
    let screencast = tokio::time::timeout(CALL_WAIT, Screencast::new())
        .await
        .map_err(|_| unavailable())?
        .context("Failed to create the screen-sharing portal")?;
    let session = tokio::time::timeout(CALL_WAIT, screencast.create_session(Default::default()))
        .await
        .map_err(|_| unavailable())?
        .context("Failed to create the capture session")?;

    // The grant kept for this window, if the folder and the name are there.
    let grants = window_kde::grant_dir();
    let name = window_kde::grant_name(&lent.uuid);
    let kept = grants.as_deref().zip(name.as_deref());
    let saved = kept.and_then(|(dir, name)| crate::portal_grant::read_named(dir, name));

    // Everything from here is undone (the session closed) if it fails.
    let picked = async {
        let mut options = SelectSourcesOptions::default()
            .set_sources(Some(ashpd::enumflags2::BitFlags::from(SourceType::Window)))
            .set_multiple(Some(false))
            .set_persist_mode(if kept.is_some() {
                PersistMode::ExplicitlyRevoked
            } else {
                PersistMode::DoNot
            })
            .set_restore_token(saved.as_deref());
        // The pointer stays out of the picture where the portal can say so.
        let modes = tokio::time::timeout(CALL_WAIT, screencast.available_cursor_modes()).await;
        if matches!(&modes, Ok(Ok(modes)) if modes.contains(CursorMode::Hidden)) {
            options = options.set_cursor_mode(Some(CursorMode::Hidden));
        }
        tokio::time::timeout(CALL_WAIT, screencast.select_sources(&session, options))
            .await
            .map_err(|_| unavailable())?
            .context("Failed to select the window")?;
        let asked = Instant::now();
        let response = tokio::time::timeout(START_WAIT, screencast.start(&session, None, Default::default()))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "No one answered the screen-sharing dialog at the computer within a minute: someone there must choose the lent window in it once"
                )
            })?
            .context("Failed to start the capture")?;
        if saved.is_some() && asked.elapsed() > Duration::from_secs(5) {
            warn!(
                "The kept grant for a lent window was not taken ({}s to start): the desktop asked again",
                asked.elapsed().as_secs()
            );
        }
        let streams = response.response().context("No response from the capture start")?;

        // The table as it is now: the dialog may have been open a while.
        let (lent, table) = lent_window(seq).await?;
        let found = portal_streams(streams.streams());
        let node_id = match window_kde::match_stream(&found, &lent, &table) {
            Ok(node) => node,
            Err(why) => {
                // A grant that brought back another window, or one picked
                // wrongly, is not kept: the dialog is asked next time.
                if let Some((dir, name)) = kept {
                    let _ = crate::portal_grant::forget_named(dir, name);
                }
                anyhow::bail!(window_kde::say_mismatch(why));
            }
        };
        if let (Some((dir, name)), Some(token)) = (kept, streams.restore_token()) {
            match crate::portal_grant::write_named(dir, name, token) {
                Ok(()) => info!("Screen-sharing grant kept for a lent window"),
                Err(e) => warn!(error = %e, "Could not keep the screen-sharing grant for a lent window"),
            }
        }
        Ok::<u32, anyhow::Error>(node_id)
    }
    .await;
    let node_id = match picked {
        Ok(node_id) => node_id,
        Err(e) => {
            let _ = session.close().await;
            return Err(e);
        }
    };
    info!("Lent KDE window capture through the portal: node_id={node_id}");

    // The session is closed when the capture ends, whichever way, and at once
    // if the capture below does not start. It watches the lease too: the
    // registry says as it changes, and a slow look backs it up.
    let closer = cancel.child_token();
    let abandon = closer.clone();
    let session_end = cancel.clone();
    let watched = lease.clone();
    tokio::spawn(async move {
        let mut changes = window_target::changes();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = closer.cancelled() => break,
                _ = changes.changed() => {}
                _ = tick.tick() => {}
            }
            if !window_target::lease_alive(&watched) {
                info!("The lent window is not lent any more: ending its capture");
                session_end.cancel();
                break;
            }
        }
        let _ = session.close().await;
    });

    // The pointer is out of the picture and no cursor feed is kept; the
    // stream is of the window, so no crop record is asked for.
    let zero_copy = match super::capture_linux::spawn_pipewire_capture_with(
        node_id,
        config.clone(),
        frame_tx,
        cancel.clone(),
        None,
        false,
    ) {
        Ok(switch) => switch,
        Err(e) => {
            abandon.cancel();
            return Err(e);
        }
    };
    Ok(CaptureSession::new(config, cancel).with_zero_copy(zero_copy))
}
