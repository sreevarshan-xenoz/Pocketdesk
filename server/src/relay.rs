//! Your own relay (roadmap N2): `pocketdesk relay` runs iroh's relay server,
//! so a phone away from home can reach the desktop without n0's public
//! relays in the path. Sessions stay end-to-end encrypted; the relay
//! forwards ciphertext between endpoints it admits.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use iroh::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, AcmeConfig, CertConfig, ClientRequest, RelayConfig, Server,
    ServerConfig, TlsConfig,
};

/// The relay's options, from `pocketdesk relay`.
#[derive(Debug, Clone, Default)]
pub struct RelayOptions {
    /// Plain HTTP port (HTTPS mode serves on 80 and 443).
    pub port: u16,
    /// HTTPS with a Let's Encrypt certificate for this name.
    pub hostname: Option<String>,
    /// The contact address Let's Encrypt asks for.
    pub email: Option<String>,
    /// Endpoint ids admitted.
    pub allow: Vec<String>,
    /// Also admit this desktop and the WAN keys its paired phones registered.
    pub allow_paired: bool,
    /// Admit anyone.
    pub open: bool,
}

/// Who may use the relay: a fixed set, and with `--allow-paired`, this
/// desktop's own id and its paired phones' registered WAN keys, re-read on
/// each connection so a newly paired phone needs no restart.
#[derive(Debug)]
pub struct Allowlist {
    fixed: HashSet<EndpointId>,
    paired: Option<PathBuf>,
    open: bool,
}

impl Allowlist {
    /// From the options; refuses a relay that would admit no one, and an
    /// id that does not parse.
    pub fn from_options(
        opts: &RelayOptions,
        state_dir: Option<PathBuf>,
        own: Option<EndpointId>,
    ) -> Result<Self> {
        if !opts.open && !opts.allow_paired && opts.allow.is_empty() {
            bail!(
                "say who may use the relay: --allow-paired (on the desktop), --allow <endpoint id> \
                 (see `pocketdesk relay-ids` on the desktop), or --open for anyone"
            );
        }
        let mut fixed = HashSet::new();
        for id in &opts.allow {
            fixed.insert(
                id.trim()
                    .parse::<EndpointId>()
                    .with_context(|| format!("not an endpoint id: {id}"))?,
            );
        }
        if opts.allow_paired {
            fixed.extend(own);
        }
        Ok(Self {
            fixed,
            paired: if opts.allow_paired { state_dir } else { None },
            open: opts.open,
        })
    }

    pub fn allows(&self, id: &EndpointId) -> bool {
        self.open
            || self.fixed.contains(id)
            || self
                .paired
                .as_ref()
                .is_some_and(|dir| paired_wan_keys(dir).contains(id))
    }
}

impl AccessControl for Allowlist {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let id = request.endpoint_id();
        if self.allows(&id) {
            Access::Allow
        } else {
            tracing::info!(endpoint = %id.fmt_short(), "Relay refused an endpoint that is not on its list");
            Access::Deny {
                reason: Some("not allowed on this relay".to_string()),
            }
        }
    }
}

/// The WAN keys this desktop's paired phones registered (device keys).
fn paired_wan_keys(state_dir: &std::path::Path) -> HashSet<EndpointId> {
    pocketdesk_core::local::device_keys::read(state_dir)
        .unwrap_or_default()
        .into_values()
        .filter_map(|keys| keys.iroh?.parse().ok())
        .collect()
}

/// This desktop's own WAN endpoint id.
pub fn own_endpoint_id() -> Result<EndpointId> {
    Ok(
        crate::iroh_endpoint::load_or_create_secret_key(&crate::iroh_endpoint::key_path()?)?
            .public(),
    )
}

/// What `pocketdesk relay-ids` prints: this desktop's id, and each paired
/// phone's registered WAN key (its id cut to 8 characters).
pub fn relay_ids() -> Result<Vec<(String, String)>> {
    let mut ids = vec![("this computer".to_string(), own_endpoint_id()?.to_string())];
    let dir = crate::state::state_dir()?;
    for (device, keys) in pocketdesk_core::local::device_keys::read(&dir).unwrap_or_default() {
        if let Some(iroh) = keys.iroh {
            let short: String = device.chars().take(8).collect();
            ids.push((format!("phone {short}"), iroh));
        }
    }
    Ok(ids)
}

/// Run the relay until Ctrl+C.
pub async fn run(opts: RelayOptions) -> Result<()> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let access = Allowlist::from_options(
        &opts,
        crate::state::state_dir().ok(),
        own_endpoint_id().ok(),
    )?;
    if opts.open {
        tracing::warn!("This relay admits anyone: strangers can use its bandwidth");
    }
    let mut relay = match &opts.hostname {
        None => RelayConfig::new(SocketAddr::from(([0, 0, 0, 0], opts.port))),
        Some(hostname) => {
            let mut relay = RelayConfig::new(SocketAddr::from(([0, 0, 0, 0], 80)));
            let mut acme = AcmeConfig::letsencrypt(true)
                .domains(vec![hostname.clone()])
                .cache_path(crate::state::state_dir()?.join("relay-acme"));
            if let Some(email) = &opts.email {
                acme = acme.contact(vec![format!("mailto:{email}")]);
            }
            relay.tls = Some(TlsConfig::new(
                SocketAddr::from(([0, 0, 0, 0], 443)),
                CertConfig::LetsEncrypt {
                    acme_config: acme,
                    server_config_builder: tokio_rustls::rustls::ServerConfig::builder()
                        .with_no_client_auth(),
                },
            ));
            relay
        }
    };
    relay.access = std::sync::Arc::new(access);
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config)
        .await
        .context("the relay did not start")?;
    match (&opts.hostname, server.http_addr()) {
        (Some(hostname), _) => println!("Relay on https://{hostname}"),
        (None, Some(addr)) => println!(
            "Relay on http://<this machine's public address>:{} (listening on {addr})",
            opts.port
        ),
        (None, None) => {}
    }
    println!(
        "Set it on the desktop: relay_urls = [\"<that URL>\"] in config.toml. Ctrl+C stops it."
    );
    tokio::signal::ctrl_c().await.ok();
    server.shutdown().await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seed: u8) -> EndpointId {
        iroh::SecretKey::from_bytes(&[seed; 32]).public()
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-relay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_relay_that_would_admit_no_one_does_not_start() {
        assert!(Allowlist::from_options(&RelayOptions::default(), None, None).is_err());
        let bad = RelayOptions {
            allow: vec!["not-an-id".into()],
            ..Default::default()
        };
        assert!(Allowlist::from_options(&bad, None, None).is_err());
    }

    #[test]
    fn only_listed_endpoints_are_admitted() {
        let opts = RelayOptions {
            allow: vec![id(1).to_string()],
            ..Default::default()
        };
        let list = Allowlist::from_options(&opts, None, None).unwrap();
        assert!(list.allows(&id(1)));
        assert!(!list.allows(&id(2)));
        let open = Allowlist::from_options(
            &RelayOptions {
                open: true,
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
        assert!(open.allows(&id(2)));
    }

    #[test]
    fn on_the_desktop_its_own_id_and_its_phones_wan_keys_are_admitted() {
        let dir = dir("paired");
        let opts = RelayOptions {
            allow_paired: true,
            ..Default::default()
        };
        let list = Allowlist::from_options(&opts, Some(dir.clone()), Some(id(9))).unwrap();
        assert!(list.allows(&id(9)), "this desktop");
        assert!(
            !list.allows(&id(3)),
            "a phone that has not registered its key"
        );
        pocketdesk_core::local::device_keys::bind_first(&dir, "phone", "tls-key").unwrap();
        pocketdesk_core::local::device_keys::register(
            &dir,
            "phone",
            pocketdesk_core::local::device_keys::Slot::Iroh,
            &id(3).to_string(),
        )
        .unwrap();
        assert!(list.allows(&id(3)), "registered since: no restart needed");
    }
}
