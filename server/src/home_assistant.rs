//! Home Assistant over MQTT (ecosystem phase 1e): this computer, its paired
//! phones and its named tasks as Home Assistant entities, through the MQTT
//! broker config.toml names (`home_assistant`, `mqtt_broker`, off by
//! default). Entities announce themselves by MQTT discovery; states are
//! retained and published when they change; a task's button starts it as a
//! phone would, an ask-first one waiting for the owner. Only metadata goes
//! to the broker: no notification, text, file name or link.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::home_assistant::{self as ha, Broker, Status};
use pocketdesk_core::local::phone_status::{self, PhoneBattery};
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS};
use serde_json::{Value, json};

/// Home Assistant's discovery prefix.
const DISCOVERY: &str = "homeassistant";
/// How often the snapshot and the config are read again.
const TICK: Duration = Duration::from_secs(2);
/// A broker away is tried again after this.
const RETRY: Duration = Duration::from_secs(5);
/// A broker that refused the connection (a wrong password, say) is asked
/// again only after this, so it does not ban the computer for trying.
const REFUSED_RETRY: Duration = Duration::from_secs(60);
/// Requests the client may queue for the connection.
const QUEUE: usize = 256;

/// This computer, as its entities name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Computer {
    /// The first 12 letters and digits of its device id, lowercase.
    pub id: String,
    /// Its host name.
    pub name: String,
}

/// A paired phone's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneState {
    pub device_id: String,
    pub battery: Option<PhoneBattery>,
    /// A control link or a session.
    pub connected: bool,
}

/// What the entities show, read once a tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub sessions: usize,
    pub cast: bool,
    pub phones: Vec<PhoneState>,
    /// The task names a phone may start.
    pub tasks: Vec<String>,
    /// Commands for paired phones' automation apps (M9).
    pub commands: Vec<CommandButton>,
}

/// A phone command Home Assistant may press (M9): only one whose phone is
/// paired and whose name and text L6 takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandButton {
    pub label: String,
    pub device_id: String,
    pub name: String,
    pub text: String,
}

/// The first `n` letters and digits of `id`, lowercase.
fn short_id(id: &str, n: usize) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(n)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// A phone's id in topics and entity ids.
pub fn phone_id(device_id: &str) -> String {
    short_id(device_id, 8)
}

impl Computer {
    pub fn new(device_id: &str, name: &str) -> Self {
        Self {
            id: short_id(device_id, 12),
            name: name.to_string(),
        }
    }

    /// Where its states go.
    pub fn base(&self) -> String {
        format!("pocketdesk/{}", self.id)
    }

    /// `online` or `offline`, retained; the last will says `offline`.
    pub fn availability(&self) -> String {
        format!("{}/availability", self.base())
    }

    /// Where the activity events go (not retained).
    pub fn events(&self) -> String {
        format!("{}/event", self.base())
    }

    /// This computer's discovery configs, to find ones left behind by an
    /// earlier run.
    pub fn own_configs(&self) -> String {
        format!("{DISCOVERY}/+/pocketdesk_{}/+/config", self.id)
    }

    /// Its phones' state topics, likewise.
    pub fn own_phone_states(&self) -> String {
        format!("{}/phone/#", self.base())
    }

    /// The task buttons' command topics, `…/task/<object>/run`.
    pub fn task_commands(&self) -> String {
        format!("{}/task/+/run", self.base())
    }

    /// The phone commands' buttons' topics, `…/command/<object>/run` (M9).
    pub fn phone_commands(&self) -> String {
        format!("{}/command/+/run", self.base())
    }

    fn config_topic(&self, component: &str, object: &str) -> String {
        format!(
            "{DISCOVERY}/{component}/pocketdesk_{}/{object}/config",
            self.id
        )
    }

    fn device(&self) -> Value {
        json!({
            "identifiers": [format!("pocketdesk_{}", self.id)],
            "name": format!("{} (Pocketdesk)", self.name),
            "manufacturer": "Pocketdesk",
            "model": "Computer",
            "sw_version": env!("CARGO_PKG_VERSION"),
        })
    }

    fn phone_device(&self, phone: &str) -> Value {
        json!({
            "identifiers": [format!("pocketdesk_{}_{phone}", self.id)],
            "name": format!("Phone {phone}"),
            "manufacturer": "Pocketdesk",
            "model": "Phone",
            "via_device": format!("pocketdesk_{}", self.id),
        })
    }

    /// An entity's discovery config: `fields`, its unique id, device and
    /// availability.
    fn entity(&self, object: &str, device: Value, fields: Value) -> String {
        let mut config = fields;
        config["unique_id"] = json!(format!("pocketdesk_{}_{object}", self.id));
        config["device"] = device;
        match config.get("availability_topic") {
            None => config["availability_topic"] = json!(self.availability()),
            // Null: an entity whose state is the availability itself.
            Some(Value::Null) => {
                if let Some(map) = config.as_object_mut() {
                    map.remove("availability_topic");
                }
            }
            Some(_) => {}
        }
        config.to_string()
    }
}

/// Each task's object id (its name in lowercase letters, digits and `_`,
/// made unique) beside its name, in order.
pub fn task_objects(names: &[String]) -> Vec<(String, String)> {
    objects("task", names)
}

/// Each phone command's object id beside the command, in order (M9):
/// `cmd_<phone>_<label>`, and a hash of the whole command when two would
/// share one. Never by place in the list (M9 review: removing one command
/// gave its id, and the automations pressing it, to the next).
pub fn command_objects(commands: &[CommandButton]) -> Vec<(String, CommandButton)> {
    let mut used = HashSet::new();
    commands
        .iter()
        .map(|command| {
            let slug = slug(&command.label, "cmd");
            let base = format!("cmd_{}_{slug}", phone_id(&command.device_id));
            let mut object = base.clone();
            if !used.insert(object.clone()) {
                object = format!("{base}_{}", command_hash(command));
                // The very same command twice: one button is enough to
                // tell apart by its place.
                let mut n = 2;
                while !used.insert(object.clone()) {
                    object = format!("{base}_{}_{n}", command_hash(command));
                    n += 1;
                }
            }
            (object, command.clone())
        })
        .collect()
}

/// A short hash of all a command is, the same in every run (FNV-1a).
fn command_hash(command: &CommandButton) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for part in [
        &command.device_id,
        &command.name,
        &command.text,
        &command.label,
    ] {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    format!("{hash:08x}")
}

/// `name` in lowercase letters, digits and `_`, or `fallback` when none.
fn slug(name: &str, fallback: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

/// `names` as object ids `<prefix>_<slug>`, made unique, beside each name.
fn objects(prefix: &str, names: &[String]) -> Vec<(String, String)> {
    let mut used = HashSet::new();
    names
        .iter()
        .map(|name| {
            let mut slug: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            slug = slug.trim_matches('_').to_string();
            if slug.is_empty() {
                slug = prefix.into();
            }
            let mut unique = slug.clone();
            let mut n = 2;
            while !used.insert(unique.clone()) {
                unique = format!("{slug}_{n}");
                n += 1;
            }
            (format!("{prefix}_{unique}"), name.clone())
        })
        .collect()
}

fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}

/// Every retained message for `snapshot`: the discovery configs and the
/// states, by topic.
pub fn messages(computer: &Computer, snapshot: &Snapshot) -> BTreeMap<String, String> {
    let base = computer.base();
    let mut out = BTreeMap::new();
    let mut entity = |component: &str, object: &str, device: Value, fields: Value| {
        out.insert(
            computer.config_topic(component, object),
            computer.entity(object, device, fields),
        );
    };
    entity(
        "binary_sensor",
        "server",
        computer.device(),
        json!({
            "name": "Pocketdesk server",
            "device_class": "connectivity",
            "state_topic": computer.availability(),
            "payload_on": "online",
            "payload_off": "offline",
            // Its own state is the availability: never "unavailable".
            "availability_topic": Value::Null,
        }),
    );
    entity(
        "sensor",
        "sessions",
        computer.device(),
        json!({
            "name": "Sessions",
            "state_topic": format!("{base}/sessions"),
            "state_class": "measurement",
            "icon": "mdi:monitor-cellphone",
        }),
    );
    entity(
        "binary_sensor",
        "cast",
        computer.device(),
        json!({
            "name": "Phone screen shown",
            "state_topic": format!("{base}/cast"),
            "icon": "mdi:cellphone-screenshot",
        }),
    );
    entity(
        "event",
        "activity",
        computer.device(),
        json!({
            "name": "Activity",
            "state_topic": computer.events(),
            "event_types": pocketdesk_core::local::events::KINDS,
            "icon": "mdi:timeline-text",
        }),
    );
    for phone in &snapshot.phones {
        let p = phone_id(&phone.device_id);
        let state = format!("{base}/phone/{p}");
        if phone.battery.is_some() {
            entity(
                "sensor",
                &format!("{p}_battery"),
                computer.phone_device(&p),
                json!({
                    "name": "Battery",
                    "device_class": "battery",
                    "unit_of_measurement": "%",
                    "state_class": "measurement",
                    "state_topic": format!("{state}/battery"),
                }),
            );
            entity(
                "binary_sensor",
                &format!("{p}_charging"),
                computer.phone_device(&p),
                json!({
                    "name": "Charging",
                    "device_class": "battery_charging",
                    "state_topic": format!("{state}/charging"),
                }),
            );
        }
        entity(
            "binary_sensor",
            &format!("{p}_connected"),
            computer.phone_device(&p),
            json!({
                "name": "Connected",
                "device_class": "connectivity",
                "state_topic": format!("{state}/connected"),
            }),
        );
    }
    for (object, name) in task_objects(&snapshot.tasks) {
        entity(
            "button",
            &object,
            computer.device(),
            json!({
                "name": name,
                "command_topic": format!("{base}/task/{object}/run"),
                "payload_press": "PRESS",
                "icon": "mdi:play",
            }),
        );
    }
    // M9: the label only; the name and text stay here.
    for (object, command) in command_objects(&snapshot.commands) {
        entity(
            "button",
            &object,
            // Under its phone, so Home Assistant shows which (M9 review).
            computer.phone_device(&phone_id(&command.device_id)),
            json!({
                "name": command.label,
                "command_topic": format!("{base}/command/{object}/run"),
                "payload_press": "PRESS",
                "icon": "mdi:cellphone-play",
            }),
        );
    }
    out.insert(format!("{base}/sessions"), snapshot.sessions.to_string());
    out.insert(format!("{base}/cast"), on_off(snapshot.cast).to_string());
    for phone in &snapshot.phones {
        let state = format!("{base}/phone/{}", phone_id(&phone.device_id));
        if let Some(battery) = phone.battery {
            out.insert(format!("{state}/battery"), battery.charge.to_string());
            out.insert(format!("{state}/charging"), on_off(battery.charging).into());
        }
        out.insert(format!("{state}/connected"), on_off(phone.connected).into());
    }
    out
}

/// The messages to send to go from `sent` to `wanted`: each new or changed
/// one, and an empty one (which removes a retained message, and an entity
/// with it) for each gone.
pub fn changes(
    sent: &BTreeMap<String, String>,
    wanted: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = sent
        .keys()
        .filter(|topic| !wanted.contains_key(*topic))
        .map(|topic| (topic.clone(), String::new()))
        .collect();
    out.extend(
        wanted
            .iter()
            .filter(|(topic, payload)| sent.get(*topic) != Some(*payload))
            .map(|(topic, payload)| (topic.clone(), payload.clone())),
    );
    // Removals, then the computer's own configs, then its phones' (whose
    // device names the computer's as `via_device`, so it is registered
    // first), then the states.
    out.sort_by_key(|(topic, payload)| {
        if payload.is_empty() {
            0
        } else if !topic.ends_with("/config") {
            3
        } else if payload.contains("\"via_device\"") {
            2
        } else {
            1
        }
    });
    out
}

/// The activity event for `event`: its kind and its phone's short id, never
/// its content.
pub fn activity(event: &pocketdesk_core::local::events::Event) -> String {
    json!({
        "event_type": event.event,
        "phone": event
            .device_id
            .as_deref()
            .filter(|id| *id != ha::HOME_ASSISTANT_DEVICE)
            .map(phone_id),
    })
    .to_string()
}

/// Whether a message on a task's command topic is a press: what Home
/// Assistant sends (`PRESS`), live. A retained one would press again at
/// every connect, and an empty one is someone clearing a retained message.
pub fn is_press(retained: bool, payload: &[u8]) -> bool {
    !retained && payload == b"PRESS"
}

/// Whether a retained message the broker hands back on connect is one of
/// this computer's that is no longer wanted: an entity removed while the
/// bridge was away (a phone unpaired, a task deleted, a restart between),
/// which an empty retained message clears.
pub fn left_behind(
    computer: &Computer,
    wanted: &BTreeMap<String, String>,
    topic: &str,
    retained: bool,
    payload: &[u8],
) -> bool {
    let ours = (topic.starts_with(&format!("{DISCOVERY}/"))
        && topic.contains(&format!("/pocketdesk_{}/", computer.id))
        && topic.ends_with("/config"))
        || topic.starts_with(&format!("{}/phone/", computer.base()));
    ours && retained && !payload.is_empty() && !wanted.contains_key(topic)
}

/// The task a command topic presses, among `snapshot`'s.
pub fn pressed_task(computer: &Computer, snapshot: &Snapshot, topic: &str) -> Option<String> {
    let prefix = format!("{}/task/", computer.base());
    let object = topic.strip_prefix(&prefix)?.strip_suffix("/run")?;
    task_objects(&snapshot.tasks)
        .into_iter()
        .find(|(o, _)| o == object)
        .map(|(_, name)| name)
}

/// The phone command a command topic presses, among `snapshot`'s (M9).
pub fn pressed_command(
    computer: &Computer,
    snapshot: &Snapshot,
    topic: &str,
) -> Option<CommandButton> {
    let prefix = format!("{}/command/", computer.base());
    let object = topic.strip_prefix(&prefix)?.strip_suffix("/run")?;
    command_objects(&snapshot.commands)
        .into_iter()
        .find(|(o, _)| o == object)
        .map(|(_, command)| command)
}

/// Send a pressed phone command (M9) as `pocketdesk phone-command` does,
/// held to the phone's Command permission; why not, when it is not sent.
fn press_command(state_dir: &Path, command: &CommandButton) -> Result<(), String> {
    use pocketdesk_core::protocol::permissions::Capability;
    let store = pocketdesk_core::protocol::kdeconnect::TrustStore::read(
        state_dir.join(pocketdesk_core::local::paths::TRUST_FILE),
    )
    .map_err(|e| format!("the paired phones could not be read: {e:#}"))?;
    // Unpaired since the last tick (M9 review).
    if !store.paired_devices().contains(&command.device_id) {
        return Err("the phone is no longer paired".into());
    }
    if !store
        .permissions(&command.device_id)
        .allows(Capability::Command)
    {
        return Err("Send commands to its automation apps is off for the phone".into());
    }
    if !within_pace(&command.device_id, std::time::Instant::now()) {
        return Err(format!(
            "more than {PRESSES_PER_MINUTE} presses a minute for the phone"
        ));
    }
    pocketdesk_core::local::commands::request(
        state_dir,
        &command.device_id,
        &command.name,
        &command.text,
        pocketdesk_core::local::pin::now_unix(),
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// The most presses a minute a phone's commands take from Home Assistant
/// (M9 review: a looping automation flooded the phone's link).
pub const PRESSES_PER_MINUTE: usize = 30;

static PRESSES: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>,
    >,
> = std::sync::LazyLock::new(Default::default);

/// Whether one more press for `device` fits in the last minute's.
fn within_pace(device: &str, now: std::time::Instant) -> bool {
    let mut presses = PRESSES.lock().unwrap_or_else(|e| e.into_inner());
    let times = presses.entry(device.to_string()).or_default();
    while times
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= std::time::Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= PRESSES_PER_MINUTE {
        return false;
    }
    times.push_back(now);
    true
}

/// The commands that become buttons: valid, for a paired phone, at most
/// [`pocketdesk_core::local::commands::MAX_PHONE_COMMANDS`].
pub fn command_buttons(
    specs: &[pocketdesk_core::local::commands::PhoneCommandSpec],
    paired: &[String],
) -> Vec<CommandButton> {
    specs
        .iter()
        .filter(|c| c.problem().is_none() && paired.contains(&c.phone))
        .take(pocketdesk_core::local::commands::MAX_PHONE_COMMANDS)
        .map(|c| CommandButton {
            label: c.label.trim().to_string(),
            device_id: c.phone.clone(),
            name: c.name.clone(),
            text: c.text.clone(),
        })
        .collect()
}

/// The snapshot from the state folder and `config`. A trust store or
/// battery file that does not read (being replaced, held by a scanner)
/// keeps `previous`'s phones and batteries: one failed read must not
/// remove every phone's entities from Home Assistant.
fn read_snapshot(state_dir: &Path, config: &Config, previous: &Snapshot) -> Snapshot {
    use pocketdesk_core::local::paths::{LIVE_FILE, TRUST_FILE};
    let mirror = std::fs::read_to_string(state_dir.join(LIVE_FILE))
        .ok()
        .and_then(|raw| pocketdesk_core::local::sessions::parse_mirror(&raw))
        // One left by an earlier run is not this one's sessions.
        .filter(|m| m.is_fresh(pocketdesk_core::local::pin::now_unix()));
    // Paired for good: a guest's time-boxed grant is not a phone of this
    // computer's.
    let paired =
        pocketdesk_core::protocol::kdeconnect::TrustStore::read(state_dir.join(TRUST_FILE))
            .map(|store| store.paired_devices())
            .unwrap_or_else(|_| {
                previous
                    .phones
                    .iter()
                    .map(|p| p.device_id.clone())
                    .collect()
            });
    let batteries = phone_status::try_read(state_dir).unwrap_or_else(|| {
        previous
            .phones
            .iter()
            .filter_map(|p| p.battery.map(|b| (p.device_id.clone(), b)))
            .collect()
    });
    let (sessions, cast, connected) = match &mirror {
        Some(m) => (
            m.sessions.len(),
            m.sessions.iter().any(|s| s.cast),
            m.sessions
                .iter()
                .filter_map(|s| s.device_id.clone())
                .chain(m.linked.iter().map(|l| l.device_id.clone()))
                .collect::<HashSet<String>>(),
        ),
        None => (0, false, HashSet::new()),
    };
    let commands = command_buttons(&config.phone_command, &paired);
    Snapshot {
        sessions,
        cast,
        phones: paired
            .into_iter()
            .map(|id| PhoneState {
                battery: batteries.get(&id).copied(),
                connected: connected.contains(&id),
                device_id: id,
            })
            .collect(),
        tasks: crate::plugins::tasks::offered_names(&config.tasks),
        commands,
    }
}

/// What the bridge connects with, from config.toml; why not, when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    broker: Broker,
    user: String,
    password: String,
}

fn settings(config: &Config) -> Option<Result<Settings, String>> {
    if !config.home_assistant {
        return None;
    }
    Some(
        ha::parse_broker(&config.mqtt_broker).map(|broker| Settings {
            broker,
            user: config.mqtt_user.clone(),
            password: config.mqtt_password.clone(),
        }),
    )
}

fn write_status(
    state_dir: &Path,
    last: &mut Option<Status>,
    connected: bool,
    error: Option<String>,
) {
    let same = last
        .as_ref()
        .is_some_and(|s| s.connected == connected && s.error == error);
    if same {
        return;
    }
    let status = Status {
        connected,
        error,
        updated_unix: pocketdesk_core::local::pin::now_unix(),
    };
    if let Err(e) = ha::write_status(state_dir, &status) {
        tracing::debug!("Could not write the Home Assistant status: {e}");
    }
    *last = Some(status);
}

/// Bridge to Home Assistant while config.toml says to, for the server's life.
pub async fn run() {
    let Ok(state_dir) = crate::state::state_dir() else {
        return;
    };
    ha::clear_status(&state_dir);
    let mut last = None;
    loop {
        let config = crate::events::current_config();
        match settings(&config) {
            None => {
                if last.take().is_some() {
                    ha::clear_status(&state_dir);
                }
                tokio::time::sleep(TICK).await;
            }
            Some(Err(why)) => {
                write_status(&state_dir, &mut last, false, Some(why));
                tokio::time::sleep(TICK).await;
            }
            Some(Ok(wanted)) => connected(&state_dir, &wanted, &mut last).await,
        }
    }
}

/// Bridge with `wanted` until config.toml says otherwise.
async fn connected(state_dir: &Path, wanted: &Settings, last: &mut Option<Status>) {
    let Some(device_id) = pocketdesk_core::local::identity::device_id(state_dir) else {
        write_status(
            state_dir,
            last,
            false,
            Some("this computer has no id yet".into()),
        );
        tokio::time::sleep(TICK).await;
        return;
    };
    let computer = Computer::new(&device_id, &crate::kde::host_device_name());
    let mut options = MqttOptions::new(
        format!("pocketdesk-{}", computer.id),
        wanted.broker.host.clone(),
        wanted.broker.port,
    );
    options.set_keep_alive(Duration::from_secs(30));
    options.set_max_packet_size(256 * 1024, 256 * 1024);
    options.set_last_will(LastWill::new(
        computer.availability(),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    if !wanted.user.is_empty() {
        options.set_credentials(wanted.user.clone(), wanted.password.clone());
    }
    let (client, mut events) = AsyncClient::new(options, QUEUE);
    // The event loop runs in a task of its own: its poll makes the whole
    // connection (DNS, TCP, CONNECT), and a tick racing it would cancel a
    // slow connect, as to a `.local` name, every 2 s.
    let (to_bridge, mut from_broker) = tokio::sync::mpsc::channel::<Result<Event, String>>(64);
    let poller = tokio::spawn(async move {
        loop {
            let event = events.poll().await;
            let wait = match &event {
                Ok(_) => None,
                Err(rumqttc::ConnectionError::ConnectionRefused(_)) => Some(REFUSED_RETRY),
                Err(_) => Some(RETRY),
            };
            let event = event.map_err(|e| match e {
                rumqttc::ConnectionError::ConnectionRefused(code) => {
                    format!(
                        "the broker refused the connection ({code:?}): check the user and password"
                    )
                }
                e => e.to_string(),
            });
            if to_bridge.send(event).await.is_err() {
                return;
            }
            if let Some(wait) = wait {
                // The next poll connects again.
                tokio::time::sleep(wait).await;
            }
        }
    });
    let _poller = AbortOnDrop(poller);
    let mut sent: BTreeMap<String, String> = BTreeMap::new();
    let mut tail = pocketdesk_core::local::events::Tail::at_end(state_dir);
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut up = false;
    let mut snapshot = Snapshot::default();
    tracing::info!(broker = %wanted.broker.host, port = wanted.broker.port, "Home Assistant: connecting to the MQTT broker");
    loop {
        tokio::select! {
            event = from_broker.recv() => match event {
                None => return,
                Some(Ok(Event::Incoming(Packet::ConnAck(_)))) => {
                    tracing::info!("Home Assistant: connected to the MQTT broker");
                    up = true;
                    write_status(state_dir, last, true, None);
                    sent.clear();
                    let _ = client.try_publish(computer.availability(), QoS::AtLeastOnce, true, "online");
                    let _ = client.try_subscribe(computer.task_commands(), QoS::AtLeastOnce);
                    let _ = client.try_subscribe(computer.phone_commands(), QoS::AtLeastOnce);
                    // The broker answers these with what it kept: entities
                    // gone since are cleared as they come.
                    let _ = client.try_subscribe(computer.own_configs(), QoS::AtLeastOnce);
                    let _ = client.try_subscribe(computer.own_phone_states(), QoS::AtLeastOnce);
                    let config = crate::events::current_config();
                    snapshot = read_snapshot(state_dir, &config, &snapshot);
                    publish(&client, &computer, &snapshot, &mut sent);
                }
                Some(Ok(Event::Incoming(Packet::Publish(message)))) => {
                    let wanted = messages(&computer, &snapshot);
                    if left_behind(&computer, &wanted, &message.topic, message.retain, &message.payload) {
                        tracing::debug!(topic = %message.topic, "Home Assistant: clearing an entity left behind");
                        let _ = client.try_publish(message.topic.clone(), QoS::AtLeastOnce, true, "");
                        continue;
                    }
                    if !is_press(message.retain, &message.payload) {
                        continue;
                    }
                    if let Some(task) = pressed_task(&computer, &snapshot, &message.topic) {
                        match crate::plugins::tasks::start_for_home_assistant(&task) {
                            Ok(()) => tracing::info!(task = %task, "Home Assistant started a task"),
                            Err(why) => tracing::info!(task = %task, "Home Assistant's start was refused: {why}"),
                        }
                    } else if let Some(command) = pressed_command(&computer, &snapshot, &message.topic) {
                        match press_command(state_dir, &command) {
                            Ok(()) => tracing::info!(label = %command.label, "Home Assistant's phone command is queued for the phone (up to a minute)"),
                            Err(why) => tracing::info!(label = %command.label, "Home Assistant's phone command was refused: {why}"),
                        }
                    }
                }
                Some(Ok(Event::Incoming(Packet::SubAck(ack)))) => {
                    if ack
                        .return_codes
                        .iter()
                        .any(|c| matches!(c, rumqttc::SubscribeReasonCode::Failure))
                    {
                        tracing::warn!("Home Assistant: the broker refused a subscription; its buttons may not work");
                        write_status(
                            state_dir,
                            last,
                            true,
                            Some("the broker refused a subscription, so its buttons may not work (its access rules?)".into()),
                        );
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    if up {
                        tracing::warn!("Home Assistant: lost the MQTT broker: {e}");
                    }
                    up = false;
                    write_status(state_dir, last, false, Some(e));
                }
            },
            _ = tick.tick() => {
                let config = crate::events::current_config();
                if settings(&config).as_ref() != Some(&Ok(wanted.clone())) {
                    if up {
                        let _ = client.try_publish(computer.availability(), QoS::AtLeastOnce, true, "offline");
                        let _ = client.try_disconnect();
                        // Let the offline and the disconnect go out: the
                        // poller reports the closed connection after them.
                        let _ = tokio::time::timeout(Duration::from_secs(2), async {
                            while let Some(Ok(_)) = from_broker.recv().await {}
                        })
                        .await;
                    }
                    tracing::info!("Home Assistant: disconnected (the settings changed)");
                    return;
                }
                let new_events = tail.poll();
                if up {
                    snapshot = read_snapshot(state_dir, &config, &snapshot);
                    publish(&client, &computer, &snapshot, &mut sent);
                    for event in &new_events {
                        let _ = client.try_publish(computer.events(), QoS::AtMostOnce, false, activity(event));
                    }
                }
            }
        }
    }
}

/// Stops the event loop's task when the bridge it serves returns.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Send what changed since `sent`, retained; what the queue cannot take
/// now is sent at the next tick.
fn publish(
    client: &AsyncClient,
    computer: &Computer,
    snapshot: &Snapshot,
    sent: &mut BTreeMap<String, String>,
) {
    let wanted = messages(computer, snapshot);
    for (topic, payload) in changes(sent, &wanted) {
        if client
            .try_publish(topic.clone(), QoS::AtLeastOnce, true, payload.clone())
            .is_err()
        {
            break;
        }
        if payload.is_empty() {
            sent.remove(&topic);
        } else {
            sent.insert(topic, payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer() -> Computer {
        Computer::new("1a2b3c4d-5e6f-7a8b-9c0d-ef0123456789", "desk")
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            sessions: 1,
            cast: false,
            phones: vec![
                PhoneState {
                    device_id: "5f1c9b3d-6e70-0000-0000-000000000001".into(),
                    battery: Some(PhoneBattery {
                        charge: 64,
                        charging: true,
                        updated_unix: 1,
                        ..Default::default()
                    }),
                    connected: true,
                },
                PhoneState {
                    device_id: "a71e00aa-0002".into(),
                    battery: None,
                    connected: false,
                },
            ],
            tasks: vec!["Back up".into(), "back-up".into(), "!!".into()],
            commands: vec![
                CommandButton {
                    label: "Lights off".into(),
                    device_id: "00000000-0000-4000-8000-0000000000c4".into(),
                    name: "lights".into(),
                    text: "off".into(),
                },
                CommandButton {
                    label: "Lights off".into(),
                    device_id: "00000000-0000-4000-8000-0000000000c4".into(),
                    name: "lights".into(),
                    text: "all".into(),
                },
            ],
        }
    }

    #[test]
    fn ids_are_short_letters_and_digits() {
        assert_eq!(computer().id, "1a2b3c4d5e6f");
        assert_eq!(phone_id("5F1C9B3D-6e70"), "5f1c9b3d");
        assert_eq!(
            computer().availability(),
            "pocketdesk/1a2b3c4d5e6f/availability"
        );
    }

    #[test]
    fn task_buttons_have_ids_of_their_own() {
        let objects = task_objects(&snapshot().tasks);
        assert_eq!(
            objects,
            vec![
                ("task_back_up".to_string(), "Back up".to_string()),
                ("task_back_up_2".to_string(), "back-up".to_string()),
                ("task_task".to_string(), "!!".to_string()),
            ]
        );
    }

    #[test]
    fn each_entity_announces_itself_with_its_device_and_state() {
        let all = messages(&computer(), &snapshot());
        let config = |component: &str, object: &str| -> Value {
            let topic =
                format!("homeassistant/{component}/pocketdesk_1a2b3c4d5e6f/{object}/config");
            serde_json::from_str(all.get(&topic).unwrap_or_else(|| panic!("{topic}"))).unwrap()
        };
        let server = config("binary_sensor", "server");
        assert_eq!(
            server["state_topic"],
            "pocketdesk/1a2b3c4d5e6f/availability"
        );
        assert!(
            server.get("availability_topic").is_none(),
            "its own state is availability"
        );
        assert_eq!(server["device"]["name"], "desk (Pocketdesk)");
        let battery = config("sensor", "5f1c9b3d_battery");
        assert_eq!(battery["device_class"], "battery");
        assert_eq!(battery["unit_of_measurement"], "%");
        assert_eq!(
            battery["availability_topic"],
            "pocketdesk/1a2b3c4d5e6f/availability"
        );
        assert_eq!(battery["device"]["via_device"], "pocketdesk_1a2b3c4d5e6f");
        assert_eq!(
            battery["unique_id"],
            "pocketdesk_1a2b3c4d5e6f_5f1c9b3d_battery"
        );
        let button = config("button", "task_back_up");
        assert_eq!(button["name"], "Back up");
        assert_eq!(
            button["command_topic"],
            "pocketdesk/1a2b3c4d5e6f/task/task_back_up/run"
        );
        let activity = config("event", "activity");
        assert!(
            activity["event_types"]
                .as_array()
                .unwrap()
                .contains(&json!("session_started"))
        );
        assert_eq!(all["pocketdesk/1a2b3c4d5e6f/sessions"], "1");
        assert_eq!(all["pocketdesk/1a2b3c4d5e6f/cast"], "OFF");
        assert_eq!(all["pocketdesk/1a2b3c4d5e6f/phone/5f1c9b3d/battery"], "64");
        assert_eq!(all["pocketdesk/1a2b3c4d5e6f/phone/5f1c9b3d/charging"], "ON");
        assert_eq!(
            all["pocketdesk/1a2b3c4d5e6f/phone/a71e00aa/connected"],
            "OFF"
        );
        assert!(
            !all.keys()
                .any(|t| t.contains("a71e00aa_battery") || t.ends_with("a71e00aa/battery")),
            "no battery entity before a report"
        );
    }

    #[test]
    fn the_computer_is_announced_before_its_phones() {
        let all = changes(&BTreeMap::new(), &messages(&computer(), &snapshot()));
        let first_phone = all
            .iter()
            .position(|(_, p)| p.contains("via_device"))
            .unwrap();
        let last_computer = all
            .iter()
            .rposition(|(t, p)| t.ends_with("/config") && !p.contains("via_device"))
            .unwrap();
        let first_state = all
            .iter()
            .position(|(t, _)| !t.ends_with("/config"))
            .unwrap();
        assert!(last_computer < first_phone && first_phone < first_state);
    }

    #[test]
    fn only_what_changed_is_sent_and_what_went_is_removed() {
        let mut sent = BTreeMap::new();
        let first = messages(&computer(), &snapshot());
        let all = changes(&sent, &first);
        assert_eq!(all.len(), first.len(), "everything at first");
        sent = first.clone();
        assert!(
            changes(&sent, &first).is_empty(),
            "nothing when nothing changed"
        );
        let mut later = snapshot();
        later.phones[0].battery.as_mut().unwrap().charge = 63;
        later.phones.pop();
        later.tasks.clear();
        let delta = changes(&sent, &messages(&computer(), &later));
        assert!(delta.contains(&(
            "pocketdesk/1a2b3c4d5e6f/phone/5f1c9b3d/battery".to_string(),
            "63".to_string()
        )));
        let removed: Vec<&String> = delta
            .iter()
            .filter(|(_, p)| p.is_empty())
            .map(|(t, _)| t)
            .collect();
        assert!(
            removed.contains(
                &&"homeassistant/binary_sensor/pocketdesk_1a2b3c4d5e6f/a71e00aa_connected/config"
                    .to_string()
            )
        );
        assert!(removed.contains(
            &&"homeassistant/button/pocketdesk_1a2b3c4d5e6f/task_back_up/config".to_string()
        ));
        assert!(removed.contains(&&"pocketdesk/1a2b3c4d5e6f/phone/a71e00aa/connected".to_string()));
    }

    #[test]
    fn a_press_names_one_of_the_tasks_and_nothing_else() {
        let c = computer();
        let s = snapshot();
        assert_eq!(
            pressed_task(&c, &s, "pocketdesk/1a2b3c4d5e6f/task/task_back_up_2/run").as_deref(),
            Some("back-up")
        );
        assert_eq!(
            pressed_task(&c, &s, "pocketdesk/1a2b3c4d5e6f/task/task_gone/run"),
            None
        );
        assert_eq!(
            pressed_task(&c, &s, "pocketdesk/other/task/task_back_up/run"),
            None
        );
        assert_eq!(
            pressed_task(&c, &s, "pocketdesk/1a2b3c4d5e6f/sessions"),
            None
        );
    }

    #[test]
    fn what_an_earlier_run_left_is_cleared_and_nothing_else() {
        let c = computer();
        let wanted = messages(&c, &snapshot());
        let gone = "homeassistant/sensor/pocketdesk_1a2b3c4d5e6f/0badf00d_battery/config";
        assert!(left_behind(&c, &wanted, gone, true, b"{}"));
        assert!(left_behind(
            &c,
            &wanted,
            "pocketdesk/1a2b3c4d5e6f/phone/0badf00d/connected",
            true,
            b"ON"
        ));
        let kept = "homeassistant/sensor/pocketdesk_1a2b3c4d5e6f/5f1c9b3d_battery/config";
        assert!(!left_behind(&c, &wanted, kept, true, b"{}"), "still wanted");
        assert!(
            !left_behind(&c, &wanted, gone, false, b"{}"),
            "a live echo, not what was kept"
        );
        assert!(
            !left_behind(&c, &wanted, gone, true, b""),
            "already cleared"
        );
        assert!(
            !left_behind(
                &c,
                &wanted,
                "homeassistant/sensor/pocketdesk_ffffffffffff/x/config",
                true,
                b"{}"
            ),
            "another computer's"
        );
        assert!(
            !left_behind(
                &c,
                &wanted,
                "pocketdesk/1a2b3c4d5e6f/task/task_x/run",
                true,
                b"PRESS"
            ),
            "not an entity"
        );
    }

    #[test]
    fn a_failed_read_keeps_the_phones_and_guests_are_not_phones() {
        use pocketdesk_core::local::paths::{PHONE_STATUS_FILE, TRUST_FILE};
        let dir = std::env::temp_dir().join(format!("pd-ha-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store =
            pocketdesk_core::protocol::kdeconnect::TrustStore::load_or_create(dir.join(TRUST_FILE))
                .unwrap();
        store.trust_device("5f1c9b3d-6e70").unwrap();
        store
            .trust_device_with_ttl("a71e00aa-0002", Duration::from_secs(900))
            .unwrap();
        let config = Config::default();
        let first = read_snapshot(&dir, &config, &Snapshot::default());
        assert_eq!(first.phones.len(), 1, "the guest is not listed");
        assert_eq!(first.phones[0].device_id, "5f1c9b3d-6e70");
        let previous = snapshot();
        std::fs::write(dir.join(TRUST_FILE), "not json").unwrap();
        std::fs::write(dir.join(PHONE_STATUS_FILE), "not json").unwrap();
        let kept = read_snapshot(&dir, &config, &previous);
        assert_eq!(kept.phones.len(), previous.phones.len(), "kept, not wiped");
        assert_eq!(kept.phones[0].battery, previous.phones[0].battery);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_live_press_starts_a_task() {
        assert!(is_press(false, b"PRESS"));
        assert!(
            !is_press(true, b"PRESS"),
            "a retained press would fire at every connect"
        );
        assert!(!is_press(false, b""), "clearing a retained message");
        assert!(!is_press(false, b"press"));
        assert!(!is_press(false, b"rm -rf /"));
    }

    #[test]
    fn an_activity_event_carries_no_content() {
        let event = pocketdesk_core::local::events::Event::new(
            "notification",
            Some("5f1c9b3d-6e70"),
            1,
            json!({ "app": "Chat" }),
            json!({ "title": "secret", "text": "secret" }),
            pocketdesk_core::local::events::EventsDetail::Full,
        );
        let payload = activity(&event);
        assert!(
            !payload.contains("secret") && !payload.contains("Chat"),
            "{payload}"
        );
        let value: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["event_type"], "notification");
        assert_eq!(value["phone"], "5f1c9b3d");
        let own = pocketdesk_core::local::events::Event::new(
            "task_started",
            Some(ha::HOME_ASSISTANT_DEVICE),
            1,
            json!({ "task": "Back up" }),
            json!({}),
            pocketdesk_core::local::events::EventsDetail::Metadata,
        );
        let value: Value = serde_json::from_str(&activity(&own)).unwrap();
        assert!(value["phone"].is_null(), "Home Assistant is not a phone");
    }

    #[test]
    fn the_bridge_runs_only_when_switched_on_with_a_broker() {
        let mut config = Config::default();
        assert_eq!(settings(&config), None, "off by default");
        config.home_assistant = true;
        assert!(matches!(settings(&config), Some(Err(_))), "no broker");
        config.mqtt_broker = "mqtt://192.0.2.5".into();
        assert_eq!(
            settings(&config).unwrap().unwrap().broker,
            Broker {
                host: "192.0.2.5".into(),
                port: 1883
            }
        );
    }

    /// M9: a phone command is a button with its label only, pressed on a
    /// topic of its own; ids are unique.
    #[test]
    fn phone_commands_are_buttons_with_their_labels_only() {
        let c = computer();
        let s = snapshot();
        let all = messages(&c, &s);
        let config: Value = serde_json::from_str(
            &all["homeassistant/button/pocketdesk_1a2b3c4d5e6f/cmd_00000000_lights_off/config"],
        )
        .unwrap();
        assert_eq!(config["name"], "Lights off");
        assert_eq!(
            config["command_topic"],
            "pocketdesk/1a2b3c4d5e6f/command/cmd_00000000_lights_off/run"
        );
        let raw = config.to_string();
        assert!(
            !raw.contains("\"lights\"") && !raw.contains("\"off\""),
            "no name or text"
        );
        let objects = command_objects(&s.commands);
        let second = objects[1].0.clone();
        assert!(second.starts_with("cmd_00000000_lights_off_") && second != objects[0].0);
        assert_eq!(
            pressed_command(
                &c,
                &s,
                &format!("pocketdesk/1a2b3c4d5e6f/command/{second}/run")
            )
            .map(|b| b.text),
            Some("all".to_string())
        );
        // M9 review: the first gone, the second keeps its own id, not the
        // first's.
        let mut fewer = s.clone();
        fewer.commands.remove(0);
        assert_eq!(
            command_objects(&fewer.commands)[0].0,
            "cmd_00000000_lights_off"
        );
        assert_eq!(
            command_objects(&s.commands),
            objects,
            "the same in every run"
        );
        // A command removed has its button removed (an empty retained message).
        let gone = changes(&all, &messages(&c, &fewer));
        assert!(gone.iter().any(|(topic, payload)| topic
            == &format!("homeassistant/button/pocketdesk_1a2b3c4d5e6f/{second}/config")
            && payload.is_empty()));
        assert_eq!(
            pressed_command(&c, &s, "pocketdesk/1a2b3c4d5e6f/command/cmd_gone/run"),
            None
        );
        assert_eq!(
            pressed_command(
                &c,
                &s,
                "pocketdesk/1a2b3c4d5e6f/task/cmd_00000000_lights_off/run"
            ),
            None
        );
        assert_eq!(
            pressed_task(&c, &s, "pocketdesk/1a2b3c4d5e6f/command/task_back_up/run"),
            None
        );
    }

    /// M9: only a paired phone's valid commands become buttons.
    #[test]
    fn only_a_paired_phone_s_valid_commands_are_offered() {
        use pocketdesk_core::local::commands::PhoneCommandSpec;
        let paired = vec!["phone-a".to_string()];
        let spec = |label: &str, phone: &str, name: &str| PhoneCommandSpec {
            label: label.into(),
            phone: phone.into(),
            name: name.into(),
            text: String::new(),
        };
        let buttons = command_buttons(
            &[
                spec("Go", "phone-a", "go"),
                spec("Away", "phone-b", "go"),
                spec(" ", "phone-a", "go"),
                spec("Bad", "phone-a", ""),
            ],
            &paired,
        );
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0].label, "Go");
        let many: Vec<_> = (0..40)
            .map(|i| spec(&format!("c{i}"), "phone-a", "go"))
            .collect();
        assert_eq!(
            command_buttons(&many, &paired).len(),
            pocketdesk_core::local::commands::MAX_PHONE_COMMANDS
        );
    }

    /// M9 review: presses for a phone are paced, a minute at a time.
    #[test]
    fn presses_are_paced_per_phone() {
        let start = std::time::Instant::now();
        for _ in 0..PRESSES_PER_MINUTE {
            assert!(within_pace("pace-test-phone", start));
        }
        assert!(!within_pace("pace-test-phone", start), "one too many");
        assert!(within_pace("pace-test-other", start), "another phone's own");
        assert!(within_pace(
            "pace-test-phone",
            start + std::time::Duration::from_secs(61)
        ));
    }
}
