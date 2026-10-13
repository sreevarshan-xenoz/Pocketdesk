use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::sleep;

pub mod discovery;
pub mod lan;
pub mod wol;
pub use discovery::{DiscoveryEvent, DiscoveryService};
pub use lan::{LanDiscoveryService, LanEvent};
pub use wol::send_wol_with_retry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub name: String,
    pub dns_name: String,
    pub ips: Vec<String>,
    pub online: bool,
}

#[derive(Debug, Default, Clone)]
pub struct TailscaleClient;

impl TailscaleClient {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub async fn get_self_ip(&self) -> Result<String> {
        let status = self.status().await?;
        status
            .self_node
            .and_then(|node| node.tailscale_ips.into_iter().next())
            .context("No Tailscale IP assigned")
    }

    pub async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        let status = self.status().await?;
        let peers = status
            .peer
            .into_values()
            .map(|peer| PeerInfo {
                name: peer
                    .host_name
                    .or(peer.dns_name.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                dns_name: peer.dns_name.unwrap_or_default(),
                ips: peer.tailscale_ips,
                online: peer.active.unwrap_or(false),
            })
            .collect();

        Ok(peers)
    }

    pub async fn is_peer_online(&self, peer_name: &str) -> Result<bool> {
        let peers = self.get_peers().await?;
        Ok(peers
            .iter()
            .any(|peer| peer.name == peer_name && peer.online))
    }

    pub async fn wait_for_ready(&self, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        loop {
            if started.elapsed() > timeout {
                bail!("Tailscale not ready within timeout")
            }

            match self.status().await {
                Ok(status) if status.backend_state.as_deref() == Some("Running") => {
                    return Ok(());
                }
                Ok(status) => {
                    tracing::debug!(
                        "Waiting for Tailscale, state={}",
                        status
                            .backend_state
                            .unwrap_or_else(|| "unknown".to_string())
                    );
                }
                Err(error) => {
                    tracing::debug!("Tailscale not ready: {}", error);
                }
            }

            sleep(Duration::from_secs(2)).await;
        }
    }

    pub async fn status_text(&self) -> Result<String> {
        let output = Command::new("tailscale").arg("status").output().await;

        match output {
            Ok(output) if output.status.success() => {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            }
            _ => {
                // Fallback to local API for status text
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(2))
                    .build()?;
                let res = client
                    .get("http://100.100.100.100:1053/localapi/v0/status")
                    .send()
                    .await;
                match res {
                    Ok(r) if r.status().is_success() => {
                        Ok("Tailscale Online (via LocalAPI)".to_string())
                    }
                    _ => bail!("tailscale status failed"),
                }
            }
        }
    }

    async fn status(&self) -> Result<TailscaleStatus> {
        if let Ok(ip) = std::env::var("POCKETDESK_TAILSCALE_IP") {
            return Ok(TailscaleStatus {
                backend_state: Some("Running".to_string()),
                self_node: Some(StatusPeer {
                    host_name: Some("pocketdesk-dev".to_string()),
                    dns_name: None,
                    tailscale_ips: vec![ip],
                    active: Some(true),
                }),
                peer: HashMap::new(),
            });
        }

        // Try CLI first (works on Linux desktop)
        let output = Command::new("tailscale")
            .arg("status")
            .arg("--json")
            .output()
            .await;

        if let Ok(output) = output
            && output.status.success()
        {
            return serde_json::from_slice::<TailscaleStatus>(&output.stdout)
                .context("failed to parse tailscale status JSON");
        }

        // Fallback: try Tailscale LocalAPI on standard localhost ports
        // Android Tailscale listens on 127.0.0.1:52552, desktop on various ports
        tracing::debug!("Tailscale CLI failed, falling back to Local API HTTP endpoint");

        let localapi_addrs = [
            "http://127.0.0.1:52552/localapi/v0/status",
            "http://127.0.0.1:50502/localapi/v0/status",
            "http://100.100.100.100:1053/localapi/v0/status",
        ];

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;

        for url in &localapi_addrs {
            match client.get(*url).send().await {
                Ok(r) if r.status().is_success() => match r.json::<TailscaleStatus>().await {
                    Ok(status) => return Ok(status),
                    Err(_) => continue,
                },
                _ => continue,
            }
        }

        // Android: try Tailscale HTTP API (available at localhost:port with API key)
        // The Tailscale Android app exposes a local HTTP API for companion apps
        let android_api_addrs: [(&str, Option<&str>); 3] = [
            ("http://127.0.0.1:52552/api/status", None),
            ("http://127.0.0.1:50502/api/status", None),
            ("http://100.100.100.100:1053/api/status", None),
        ];

        for (url, _api_key) in android_api_addrs {
            match client.get(url).send().await {
                Ok(r) if r.status().is_success() => {
                    // Try to parse as Tailscale API response
                    match r.json::<serde_json::Value>().await {
                        Ok(json) => {
                            // Parse the Tailscale Android API format
                            if let Some(peer_list) = json
                                .get("Peer")
                                .or(json.get("peer"))
                                .and_then(|v| v.as_array())
                            {
                                let mut peers = HashMap::new();
                                for p in peer_list {
                                    let name = p
                                        .get("DNSName")
                                        .or(p.get("dnsName"))
                                        .or(p.get("HostName"))
                                        .or(p.get("hostName"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.trim_end_matches('.').to_string())
                                        .unwrap_or_else(|| "unknown".to_string());
                                    let ips: Vec<String> = p
                                        .get("TailscaleIPs")
                                        .or(p.get("tailscaleIPs"))
                                        .and_then(|v| v.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|ip| ip.as_str().map(String::from))
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let online = p
                                        .get("Online")
                                        .or(p.get("online"))
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    let dns_name = p
                                        .get("DNSName")
                                        .or(p.get("dnsName"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from)
                                        .unwrap_or_default();
                                    let host_name = p
                                        .get("HostName")
                                        .or(p.get("hostName"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from);

                                    if !ips.is_empty() {
                                        peers.insert(
                                            name.clone(),
                                            StatusPeer {
                                                host_name,
                                                dns_name: Some(dns_name),
                                                tailscale_ips: ips,
                                                active: Some(online),
                                            },
                                        );
                                    }
                                }

                                // Get self info
                                let self_node = json.get("Self").or(json.get("self")).map(|s| {
                                    let ips: Vec<String> = s
                                        .get("TailscaleIPs")
                                        .or(s.get("tailscaleIPs"))
                                        .and_then(|v| v.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|ip| ip.as_str().map(String::from))
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    StatusPeer {
                                        host_name: s
                                            .get("HostName")
                                            .or(s.get("hostName"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                        dns_name: s
                                            .get("DNSName")
                                            .or(s.get("dnsName"))
                                            .and_then(|v| v.as_str())
                                            .map(String::from),
                                        tailscale_ips: ips,
                                        active: Some(true),
                                    }
                                });

                                return Ok(TailscaleStatus {
                                    backend_state: Some("Running".to_string()),
                                    self_node,
                                    peer: peers,
                                });
                            }
                        }
                        Err(_) => continue,
                    }
                }
                _ => continue,
            }
        }

        // As a last resort, try to parse tailscale net status (network check)
        // This helps determine if tailscale is running even without API access
        let ts_net = Command::new("tailscale")
            .args(["netcheck", "--json"])
            .output()
            .await;

        if let Ok(output) = ts_net
            && output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("DNSChecked")
        {
            tracing::debug!("Tailscale network stack is active");
            return Ok(TailscaleStatus {
                backend_state: Some("Running".to_string()),
                self_node: None,
                peer: HashMap::new(),
            });
        }

        // If all methods failed (common on Android without root),
        // return empty peers gracefully instead of failing hard.
        tracing::warn!(
            "Tailscale peer discovery unavailable on this platform; returning empty peers"
        );
        Ok(TailscaleStatus {
            backend_state: Some("Running".to_string()),
            self_node: None,
            peer: HashMap::new(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct TailscaleStatus {
    #[serde(rename = "BackendState")]
    backend_state: Option<String>,
    #[serde(rename = "Self")]
    self_node: Option<StatusPeer>,
    #[serde(rename = "Peer", default, deserialize_with = "null_as_default")]
    peer: HashMap<String, StatusPeer>,
}

/// `tailscale status --json` writes `null`, not an empty collection, for
/// lists it has nothing in — `Peer` and `TailscaleIPs` while the daemon is
/// stopped — and `#[serde(default)]` only covers a field that is absent.
fn null_as_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
struct StatusPeer {
    #[serde(rename = "HostName")]
    host_name: Option<String>,
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
    #[serde(rename = "TailscaleIPs", default, deserialize_with = "null_as_default")]
    tailscale_ips: Vec<String>,
    #[serde(rename = "Active")]
    active: Option<bool>,
}

#[cfg(test)]
mod status_json_tests {
    use super::*;

    /// The shape `tailscale status --json` has while the daemon is stopped
    /// (fields trimmed): empty lists come out as `null`. Parsing it used to
    /// fail every discovery tick with a warning.
    #[test]
    fn a_stopped_daemons_null_lists_parse_as_empty() {
        let json = r#"{
            "BackendState": "NoState",
            "TailscaleIPs": null,
            "Self": {"HostName": "desk", "DNSName": "", "TailscaleIPs": null, "Active": false},
            "Peer": null
        }"#;
        let status: TailscaleStatus = serde_json::from_str(json).expect("stopped status parses");
        assert_eq!(status.backend_state.as_deref(), Some("NoState"));
        assert!(status.peer.is_empty());
        assert!(status.self_node.expect("self").tailscale_ips.is_empty());
    }
}
