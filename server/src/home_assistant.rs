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

    /// The task buttons' command topics, `…/task/<object>/run`.
    pub fn task_commands(&self) -> String {
        format!("{}/task/+/run", self.base())
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
                slug = "task".into();
            }
            let mut unique = slug.clone();
            let mut n = 2;
            while !used.insert(unique.clone()) {
                unique = format!("{slug}_{n}");
                n += 1;
            }
            (format!("task_{unique}"), name.clone())
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

/// The task a command topic presses, among `snapshot`'s.
pub fn pressed_task(computer: &Computer, snapshot: &Snapshot, topic: &str) -> Option<String> {
    let prefix = format!("{}/task/", computer.base());
    let object = topic.strip_prefix(&prefix)?.strip_suffix("/run")?;
    task_objects(&snapshot.tasks)
        .into_iter()
        .find(|(o, _)| o == object)
        .map(|(_, name)| name)
}

/// The snapshot from the state folder and `config`.
fn read_snapshot(state_dir: &Path, config: &Config) -> Snapshot {
    use pocketdesk_core::local::paths::{LIVE_FILE, TRUST_FILE};
    let mirror = std::fs::read_to_string(state_dir.join(LIVE_FILE))
        .ok()
        .and_then(|raw| pocketdesk_core::local::sessions::parse_mirror(&raw))
        // One left by an earlier run is not this one's sessions.
        .filter(|m| m.is_fresh(pocketdesk_core::local::pin::now_unix()));
    let paired =
        pocketdesk_core::protocol::kdeconnect::TrustStore::read(state_dir.join(TRUST_FILE))
            .map(|store| store.trusted_devices())
            .unwrap_or_default();
    let batteries = phone_status::read(state_dir);
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
            let failed = event.is_err();
            if to_bridge
                .send(event.map_err(|e| e.to_string()))
                .await
                .is_err()
            {
                return;
            }
            if failed {
                // The next poll connects again.
                tokio::time::sleep(RETRY).await;
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
                    let config = crate::events::current_config();
                    snapshot = read_snapshot(state_dir, &config);
                    publish(&client, &computer, &snapshot, &mut sent);
                }
                Some(Ok(Event::Incoming(Packet::Publish(message)))) => {
                    if !is_press(message.retain, &message.payload) {
                        continue;
                    }
                    if let Some(task) = pressed_task(&computer, &snapshot, &message.topic) {
                        match crate::plugins::tasks::start_for_home_assistant(&task) {
                            Ok(()) => tracing::info!(task = %task, "Home Assistant started a task"),
                            Err(why) => tracing::info!(task = %task, "Home Assistant's start was refused: {why}"),
                        }
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
                    snapshot = read_snapshot(state_dir, &config);
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
}
