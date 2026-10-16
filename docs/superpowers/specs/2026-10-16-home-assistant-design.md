# Home Assistant over MQTT (ecosystem phase 1e)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1). Home Assistant users want their computer and phones in dashboards and automations: the phone's battery, whether it is connected, whether someone is using the computer, and a button for each task they named (1c). The desktop ↔ Home Assistant niche is open: HASS.Agent sees little activity, and KDE's integration is a pre-alpha.

## What it does

- **Off by default.** Desktop Settings "Home Assistant" sets four config.toml keys; config.toml is owner-only:
  - `home_assistant` (false), the switch;
  - `mqtt_broker`, the broker's address (`mqtt://host:1883`, or `host[:port]`, port 1883 by default);
  - `mqtt_user` and `mqtt_password`, optional.
- **Connection:** the server connects to that broker as an MQTT 3.1.1 client (rumqttc, plain TCP), id `pocketdesk-<computer>`.
  - Reconnects: it retries every 5 s while the broker is away.
  - Availability: its last will publishes `offline` (retained) to `pocketdesk/<computer>/availability`; once connected it publishes `online`.
  - Config: changes are read within 2 s; a changed broker, user or password reconnects, and the switch off disconnects (publishing `offline` first).
- **Status:** the server writes `<state>/home_assistant.json` (`{connected, error, updated_unix}`), which Settings shows.
- **Discovery:** each entity announces itself through Home Assistant's MQTT discovery (`homeassistant/<component>/pocketdesk_<computer>/<object>/config`, retained), so it appears with no YAML.
  - **The computer** (a device named "<computer name> (Pocketdesk)"):
    - "Pocketdesk server" (binary sensor, connectivity: the availability topic itself);
    - "Sessions" (sensor, the number of live sessions);
    - "Phone screen shown" (binary sensor, a phone's cast running);
    - "Activity" (an event entity): each event's kind, and the phone's short id when it has one. Metadata only, whatever `events_detail` says: no content ever goes to the broker. Needs the event log (`events`, on by default).
  - **Each paired phone** (its own device "Phone <id>", via the computer):
    - "Battery" (sensor, %, device class battery), once it has reported one;
    - "Charging" (binary sensor, battery_charging);
    - "Connected" (binary sensor, connectivity: a control link or a session).
  - **Each named task** (1c): a button "<task>" that starts it as a phone would.
    - **Ask-first still asks** at the desk: the owner's yes is never assumed for a broker. The prompt says Home Assistant asked.
    - The same limits: one of each task and two in all at once, and 10 starts a minute from Home Assistant.
    - Its run is logged as events (`task_started`, `task_finished` or `task_refused`, the device `home-assistant`).
- **Removal:** an entity that goes away (a phone unpaired, a task removed) gets an empty retained config, which removes it from Home Assistant.
- **State topics:** `pocketdesk/<computer>/…`, retained, published when a value changes and all again on each connect.
- **Ids:**
  - the computer is the first 12 letters and digits of its Pocketdesk device id;
  - a phone, the first 8 of its id;
  - a task's object id is its name in lowercase letters, digits and `_`, made unique.

## Not in 1e

- TLS to the broker (`mqtts://`; refused with a reason).
- Lock and sleep buttons (power actions).
- The phone as an MQTT client of its own.
- A discovery prefix other than `homeassistant`.

## Checks

- **Rust:**
  - the broker address parse;
  - the discovery payloads and topics (pure, `server/src/home_assistant.rs`);
  - the states from snapshots;
  - removals;
  - the task object ids;
  - a button press reaching the tasks plugin with ask-first kept;
  - the config keys saved and read.
- **Device:** a local broker (amqtt or mosquitto) and a subscriber on this box, with the A71:
  - entities announced;
  - the A71's battery and connection published;
  - a task button run, an ask-first one held for the owner;
  - the switch off publishing `offline`.

  Home Assistant itself is owed unless a test instance is at hand.

## As built

- **The event loop runs in a task of its own** that forwards rumqttc's events: its poll makes the whole connection, and a 2 s tick racing it would cut short any connect slower than that (a `.local` name).
- **Presses:** only a live `PRESS` starts a task; a retained message or an empty one (a client clearing a retained message) does not.
- **Entities removed while away** (a phone unpaired or a task deleted while the broker was down, Home Assistant off, or across a restart) are cleared at the next connect: the bridge subscribes to its own discovery configs and phone states and clears each retained one it no longer wants.
- **Phones** are those paired for good, not guests' time-boxed grants; a trust store or battery file that does not read keeps the last good list.
- **`home-assistant`** is refused as a phone's device id (`valid_device_id`), so no phone's start reads as Home Assistant's.
- **Order:** removals, then the computer's configs, then its phones' (whose `via_device` names the computer), then the states.
- **Refusals:** a broker refusing the connection is asked again after a minute, and Settings says to check the user and password; a refused subscription shows as "Connected to the broker, but …". Settings reads the status only while the server runs.
- **Switching off** marks the entities unavailable (the bridge publishes `offline`); they stay in Home Assistant until its device is deleted there.
