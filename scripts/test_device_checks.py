"""The device checks runner's pure parts (N1)."""

import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))

import device_checks as dc  # noqa: E402


class PureParts(unittest.TestCase):
    def test_linked_phones_from_the_mirror(self):
        mirror = '{"updated_unix": 1, "linked": [{"device_id": "aaa", "files": true}, {"files": false}]}'
        self.assertEqual(dc.linked_ids(mirror), ["aaa"])
        self.assertEqual(dc.linked_ids("not json"), [])

    def test_events_of_a_kind_since_a_time(self):
        lines = [
            '{"at": 10, "event": "notification"}',
            '{"at": 20, "event": "notification", "phone_in_use": true}',
            '{"at": 30, "event": "phone_linked"}',
            "garbage",
        ]
        got = dc.events_after(lines, 15, "notification")
        self.assertEqual(len(got), 1)
        self.assertTrue(got[0]["phone_in_use"])

    def test_the_drive_s_folders(self):
        xml = (
            "<D:multistatus><D:response><D:href>/t/</D:href></D:response>"
            "<D:response><D:href>/t/Pixel%20%28ab%29/</D:href></D:response>"
            "<D:response><D:href>/t/Pixel%20%28ab%29/Docs/</D:href></D:response></D:multistatus>"
        )
        self.assertEqual(dc.dav_children(xml, "/t/"), ["Pixel (ab)"])
        self.assertEqual(dc.drive_root(["Docs (1)", "Docs (2)", "DCIM"], "Docs"), "Docs (1)")
        self.assertEqual(dc.drive_root(["DCIM"], "Docs"), None)
        self.assertEqual(dc.drive_root(["Docs"], None), None)

    def test_the_marked_folder(self):
        prefs = '[{"id": 0, "name": "DCIM"}, {"id": 1, "name": "Docs", "add": true}]'
        self.assertEqual(dc.marked_folder(prefs), ("Docs", "DCIM"))
        self.assertEqual(dc.marked_folder('[{"id": 0, "name": "DCIM"}]'), (None, "DCIM"))
        self.assertEqual(dc.marked_folder("not json"), (None, None))

    def test_a_media_session_shown(self):
        dump = (
            "MEDIA SESSION SERVICE (dumpsys media_session)\n"
            "  Sessions Stack - have 2 sessions:\n"
            "    Pocketdesk computer dev.pocketdesk.android/Pocketdesk computer (userId=0)\n"
            "      package=dev.pocketdesk.android\n"
            "      active=true\n"
            "      metadata: size=3, description=N5 check 1, N5 artist, N5 player\n"
            "    Player dev.pocketdesk.fixture/Player (userId=0)\n"
            "      package=dev.pocketdesk.fixture\n"
            "      active=true\n"
            "      metadata: size=2, description=Other song, Artist\n"
        )
        self.assertTrue(dc.media_session_shows(dump, "dev.pocketdesk.android", "N5 check 1"))
        self.assertFalse(dc.media_session_shows(dump, "dev.pocketdesk.android", "Other song"))
        self.assertFalse(dc.media_session_shows(dump.replace("active=true", "active=false", 1), "dev.pocketdesk.android", "N5 check 1"))

    def test_the_fixture_field_s_text(self):
        dump = (
            '<hierarchy><node index="0" text="other" content-desc="" />'
            '<node index="1" text="H&#233;llo &amp; t" content-desc="fixture-field" /></hierarchy>'
        )
        self.assertEqual(dc.field_text(dump), "Héllo & t")
        self.assertIsNone(dc.field_text("<hierarchy/>"))

    def test_shared_preferences(self):
        xml = (
            "<map><boolean name=\"enabled\" value=\"true\" /><string name=\"x\">a&amp;b</string>"
            "<set name=\"shared\"><string>dev.pocketdesk.fixture</string></set></map>"
        )
        got = dc.prefs(xml)
        self.assertIs(got["enabled"], True)
        self.assertEqual(got["x"], "a&b")
        self.assertEqual(got["shared"], ["dev.pocketdesk.fixture"])

    def test_the_owner_s_own_state_folder_is_refused(self):
        owners = [pathlib.Path("C:/Users/x/AppData/Local/pocketdesk")]
        self.assertTrue(dc.is_owner_root(pathlib.Path("C:/Users/x/AppData/Local/pocketdesk"), owners))
        self.assertTrue(dc.is_owner_root(pathlib.Path("C:/Users/x/AppData/Local"), [pathlib.Path("C:/Users/x/AppData/Local/state")]))
        self.assertFalse(dc.is_owner_root(pathlib.Path("C:/tmp/srv"), owners))

    def test_a_stale_mirror_links_no_one(self):
        mirror = '{"updated_unix": 100, "linked": [{"device_id": "aaa"}]}'
        self.assertEqual(dc.fresh_linked(mirror, 105), ["aaa"])
        self.assertEqual(dc.fresh_linked(mirror, 200), [])

    def test_the_server_s_status_past_its_log_lines(self):
        out = 'INFO conn{id=1}: x\n{"api": 1, "server": {"running": true}}\n'
        self.assertTrue(dc.status_running(out))
        self.assertFalse(dc.status_running('{"api": 1, "server": {"running": false}}'))
        self.assertFalse(dc.status_running("no server"))

    def test_device_commands_keep_empty_and_spaced_arguments(self):
        self.assertEqual(dc.device_command("am", "--es", "text", ""), "am --es text ''")
        self.assertEqual(dc.device_command("--es", "text", "In hand"), "--es text 'In hand'")

    def test_the_control_port(self):
        self.assertEqual(dc.control_port("control_port = 1726\nstreaming_port = 4726\n"), 1726)
        self.assertEqual(dc.control_port(""), 1716)

    def test_config_keys_set_at_the_top(self):
        toml = (
            'control_port = 1726\nhold_words = [\n  "a",\n  "b",\n]\nquiet_hours = false\n'
            "[hooks]\nquiet_hours = 1\n"
        )
        got = dc.with_config(toml, {"quiet_hours": "true", "hold_words": '["p4"]'})
        self.assertTrue(got.startswith('quiet_hours = true\nhold_words = ["p4"]\ncontrol_port = 1726\n'), got)
        self.assertNotIn('"a"', got)
        # A key under a table is another key.
        self.assertIn("[hooks]\nquiet_hours = 1\n", got)
        self.assertEqual(dc.control_port(got), 1726)

    def test_config_arrays_end_at_their_own_line(self):
        # A `]` in a string ends nothing; a kept array's lines are not tables.
        toml = (
            'hold_words = [\n  "Signal: [promo]",\n  "x",\n]\n'
            'deck = [\n  [1, 2],\n]\nquiet_hours = false\n'
        )
        got = dc.with_config(toml, {"hold_words": '["p4"]', "quiet_hours": "true"})
        self.assertEqual(got, 'hold_words = ["p4"]\nquiet_hours = true\ndeck = [\n  [1, 2],\n]\n')
        # One on a line, or begun on its key's line.
        got = dc.with_config('hold_words = ["a"]\nquiet_days = ["mon",\n "tue",\n]\nx = 1\n', {"quiet_days": "[]"})
        self.assertEqual(got, 'quiet_days = []\nhold_words = ["a"]\nx = 1\n')

    def test_pocketdesk_s_alarm_vibrations(self):
        dump = (
            "  Previous vibrations for usage ALARM:\n"
            "    createTime: 10-22 22:22:00.475, startTime: 10-22 22:22:00.477, durationMs: 77050, "
            "status: cancelled_by_user, callerInfo: CallerInfo{ attrs=VibrationAttributes: Usage=ALARM "
            "uid=1, opPkg=com.coloros.alarmclock, reason=null}\n"
            "    createTime: 10-23 14:39:06.370, startTime: 10-23 14:39:06.376, durationMs: 30009, "
            "status: cancelled_by_user, callerInfo: CallerInfo{ attrs=VibrationAttributes: Usage=ALARM "
            "uid=2, opPkg=dev.pocketdesk.android, reason=null}\n"
        )
        self.assertEqual(dc.alarm_vibrations(dump, "dev.pocketdesk.android"), [("10-23 14:39:06", 30009)])

    def test_torch_turns_since_a_time(self):
        dump = (
            "  10-23 14:39:36 : Torch for camera id 0 turned off for client PID 1\n"
            "  10-23 14:39:35 : Torch for camera id 0 turned on for client PID 1\n"
            "  10-23 14:39:05 : Torch for camera id 0 turned on for client PID 1\n"
        )
        self.assertEqual(
            dc.torch_turns(dump, "10-23 14:39:06"),
            [("10-23 14:39:35", "on"), ("10-23 14:39:36", "off")],
        )

    def test_every_check_names_a_checklist_section(self):
        for cid, (section, _) in dc.CHECKS.items():
            # Some sections carry a letter (2d, 11b).
            self.assertRegex(section, r"^§\d+[a-z]?$", cid)



class SessionParts(unittest.TestCase):
    """W1: what the session checks read from a UI dump and the phone's log."""

    DUMP = (
        '<hierarchy><node text="Test Desk" bounds="[60,1490][700,1540]" />'
        '<node text="127.0.0.1 · Paired" bounds="[60,1550][700,1600]" />'
        '<node text="Connect" bounds="[60,1670][300,1740]" />'
        '<node text="Test Desk" bounds="[60,2040][700,2090]" />'
        '<node text="192.0.2.5 · Paired" bounds="[60,2100][700,2150]" />'
        '<node text="Connect" bounds="[60,2220][300,2290]" />'
        '<node text="rates over 2.5 s · rtt median of 6" bounds="[0,0][9,9]" /></hierarchy>'
    )

    def test_the_connect_button_of_a_card(self):
        self.assertEqual(dc.connect_button(self.DUMP, "127.0.0.1"), (180, 1705))
        self.assertEqual(dc.connect_button(self.DUMP, "192.0.2.5"), (180, 2255))
        self.assertIsNone(dc.connect_button(self.DUMP, "198.51.100.1"))

    def test_the_hud_caption_and_its_rtt_count(self):
        caption = dc.hud_caption(self.DUMP)
        self.assertEqual(caption, "rates over 2.5 s · rtt median of 6")
        self.assertEqual(dc.rtt_count(caption), 6)
        self.assertEqual(dc.rtt_count("rates over 1.0 s · rtt not measured yet"), 0)
        self.assertIsNone(dc.rtt_count("fps 12"))
        self.assertIsNone(dc.hud_caption("<hierarchy />"))

    def test_links_by_the_internet_road_in_a_coloured_log(self):
        esc = chr(27)
        coloured = f'INFO conn{{{esc}[3mtransport{esc}[0m{esc}[2m={esc}[0m"iroh"}}: x: Entering main packet loop'
        home = 'INFO conn{transport="tcp"}: x: Entering main packet loop'
        self.assertEqual(dc.iroh_loops(coloured + chr(10) + home), 1)

    def test_bitrate_changes_in_the_server_s_log(self):
        esc = chr(27)
        log = chr(10).join([
            f"INFO x: Encoder bitrate target changed {esc}[3mloss_cap{esc}[0m=800000",
            "INFO x: Adaptive bitrate: 4000000 bps",
            "INFO x: Window capture target changed",
        ])
        self.assertEqual(dc.bitrate_changes(log), 2)

    def test_the_history_sheet_s_rows(self):
        dump = (
            '<hierarchy><node text="Clipboard history" clickable="false" bounds="[0,0][9,72]" />'
            '<node text="Clear" clickable="true" bounds="[0,80][9,140]" />'
            '<node text="a copy" clickable="true" bounds="[0,200][9,344]" />'
            '<node text="" clickable="true" bounds="[0,400][9,544]" />'
            '<node text="another" clickable="true" bounds="[0,600][9,621]" /></hierarchy>'
        )
        self.assertEqual(dc.history_rows(dump), [144, 21])

    def test_copies_sent_to_a_phone(self):
        esc = chr(27)
        log = chr(10).join([
            f"INFO x: Clipboard sent to phone-a ({esc}[1m15 chars)",
            "INFO x: Clipboard sent to phone-a (15 chars)",
            "INFO x: Clipboard sent to phone-b (15 chars)",
            "INFO x: Clipboard sent to phone-a (9 chars)",
        ])
        self.assertEqual(dc.pushes(log, "phone-a", 15), 2)

    def test_a_logcat_line_s_time(self):
        line = "10-24 10:47:56.619 23512 23723 I PocketdeskRust:  INFO connected"
        self.assertAlmostEqual(dc.log_seconds(line), 10 * 3600 + 47 * 60 + 56.619, places=3)
        self.assertIsNone(dc.log_seconds("--------- beginning of main"))

    def test_a_share_continued_by_the_computer(self):
        esc = chr(27)
        log = chr(10).join([
            "INFO x: Receiving ac1-a.bin from iroh (100 bytes)",
            f"INFO x: Continued ac1-a.bin from {esc}[1m30 of 100 bytes",
            "INFO x: Continued other.bin from 5 of 9 bytes",
        ])
        self.assertEqual(dc.continued_from(log, "ac1-a.bin"), (30, 100))
        self.assertIsNone(dc.continued_from(log, "ac1-b.bin"))

    def test_the_partial_a_share_left(self):
        import tempfile
        with tempfile.TemporaryDirectory() as name:
            folder = pathlib.Path(name)
            self.assertEqual(dc.partial_bytes(folder / "nothing"), 0)
            (folder / "a.part").write_bytes(b"x" * 7)
            (folder / "b.part").write_bytes(b"x" * 3)
            (folder / "index.json").write_bytes(b"x" * 50)
            self.assertEqual(dc.partial_bytes(folder), 7)
            self.assertEqual(dc.part_names(folder), frozenset({"a.part", "b.part"}))
            self.assertEqual(dc.partial_bytes(folder, frozenset({"a.part"})), 3)

    def test_a_changed_start_keeps_size_and_date(self):
        import os
        import tempfile
        with tempfile.TemporaryDirectory() as name:
            path = pathlib.Path(name) / "f.bin"
            path.write_bytes(bytes(3 << 20))
            os.utime(path, ns=(1_000_000_000_000_000_000, 1_600_000_000_123_456_789))
            before = path.stat()
            dc.change_start(path)
            after = path.stat()
            self.assertEqual(after.st_size, before.st_size)
            self.assertEqual(after.st_mtime_ns, before.st_mtime_ns)
            data = path.read_bytes()
            self.assertNotEqual(data[:1 << 20], bytes(1 << 20))
            self.assertEqual(data[1 << 20:], bytes(2 << 20))

    def test_how_a_received_file_is_dated(self):
        original, arrived = 1_577_968_200, 1_793_000_000
        self.assertEqual(dc.dated_as(original + 1, original, arrived), "kept")
        self.assertEqual(dc.dated_as(arrived - 30, original, arrived), "arrival")
        self.assertEqual(dc.dated_as(original + 86_400, original, arrived), "other")

    def test_the_phone_s_main_log_buffer(self):
        said = chr(10).join([
            "main: ring buffer is 256 KiB (255 KiB consumed, 868 KiB readable), max entry is 5120 B",
            "system: ring buffer is 1 MiB (248 KiB consumed), max entry is 5120 B",
        ])
        self.assertEqual(dc.main_log_size(said), "256K")
        self.assertEqual(dc.main_log_size("main: ring buffer is 16 MiB (1 KiB consumed)"), "16M")
        self.assertIsNone(dc.main_log_size("error: no such buffer"))
        self.assertEqual(dc.kib("256K"), 256)
        self.assertEqual(dc.kib("16M"), 16384)

    def test_a_made_png(self):
        import zlib
        png = dc.made_png(b"seed")
        self.assertEqual(png[:8], bytes([137, 80, 78, 71, 13, 10, 26, 10]))
        self.assertNotEqual(png, dc.made_png(b"other"))
        # Its one IDAT holds 16 rows of a filter byte and 16 RGB pixels.
        at = png.index(b"IDAT")
        size = int.from_bytes(png[at - 4:at], "big")
        self.assertEqual(len(zlib.decompress(png[at + 4:at + 4 + size])), 16 * (1 + 16 * 3))

    def test_images_told_to_a_phone(self):
        esc = chr(27)
        log = chr(10).join([
            f"INFO x: Clipboard image told to phone-a ({esc}[1m4096 bytes)",
            "INFO x: Clipboard image told to phone-b (4096 bytes)",
            "INFO x: Clipboard sent to phone-a (5 chars)",
        ])
        self.assertEqual(dc.images_told(log, "phone-a"), 1)

    def test_images_in_the_phone_s_clipboard_cache(self):
        listing = chr(10).join(["a" * 64 + ".png", "b" * 64 + ".part", "notes.txt", "C" * 64 + ".png"])
        self.assertEqual(dc.cached_images(listing), {"a" * 64})

    def test_the_window_picker_s_rows(self):
        dump = (
            '<hierarchy><node text="Stream a window" bounds="[0,0][9,60]" />'
            '<node text="Whole desktop" bounds="[0,80][9,140]" />'
            '<node text="A title" bounds="[0,200][9,240]" />'
            '<node text="app-a  ·  1280x720  ·  focused" bounds="[0,240][1080,280]" />'
            '<node text="app-b  ·  ws 2  ·  640x480" bounds="[0,320][1080,360]" /></hierarchy>'
        )
        self.assertEqual(dc.window_rows(dump), [(1280, 720, 540, 260), (640, 480, 540, 340)])

    def test_a_window_to_crop_to(self):
        rows = [(2560, 1440, 1, 1), (1395, 786, 2, 2), (300, 200, 3, 3), (800, 600, 4, 4)]
        self.assertEqual(dc.a_window(rows, (2560, 1440)), (800, 600, 4, 4))
        self.assertEqual(dc.a_window([(2560, 1440, 1, 1)], (2560, 1440)), None)
        self.assertEqual(dc.a_window([(1395, 786, 2, 2)], None), (1395, 786, 2, 2))
        esc = chr(27)
        log = f"INFO capture: WGC capture started {esc}[3mwidth{esc}[0m=2560 height=1440"
        self.assertEqual(dc.capture_size(log), (2560, 1440))
        self.assertIsNone(dc.capture_size("nothing"))

    def test_encoder_rebuilds_in_the_server_s_log(self):
        esc = chr(27)
        log = chr(10).join([
            f'INFO x: {esc}[3mwidth{esc}[0m=1280 height=720 backend="mf" codec="H.265 (HEVC)" Encoder rebuilt at frame resolution',
            "INFO x: width=1920 height=1080 backend=sw codec=H.264 Encoder rebuilt at frame resolution",
            "INFO x: Encoder rebuilt for new bitrate",
        ])
        self.assertEqual(dc.rebuilds(log), [(1280, 720, "H.265 (HEVC)"), (1920, 1080, "H.264")])

    def test_the_session_records(self):
        import tempfile
        with tempfile.TemporaryDirectory() as name:
            root = pathlib.Path(name)
            self.assertEqual(dc.session_records(root), [])
            (root / "state").mkdir()
            tab = chr(9)
            lines = [tab.join(["outcome=wan_punched", "rtt_p50=4", "rtt_max=5"]), "", tab.join(["outcome=lan", "enc_max=9"])]
            (root / "state" / "streaming_sessions.log").write_text(chr(10).join(lines), encoding="utf-8")
            self.assertEqual(dc.session_records(root), [
                {"outcome": "wan_punched", "rtt_p50": "4", "rtt_max": "5"},
                {"outcome": "lan", "enc_max": "9"},
            ])

    def test_a_switch_on_a_row(self):
        dump = (
            '<hierarchy><node text="Phone mic share" checkable="false" checked="false" bounds="[100,500][600,560]" />'
            '<node text="" checkable="true" checked="true" bounds="[900,505][1000,555]" />'
            '<node text="Clipboard sync" checkable="false" checked="false" bounds="[100,700][600,760]" />'
            '<node text="" checkable="true" checked="false" bounds="[900,705][1000,755]" /></hierarchy>'
        )
        self.assertIs(dc.switch_on_row(dump, "Phone mic share"), True)
        self.assertIs(dc.switch_on_row(dump, "Clipboard sync"), False)
        self.assertIsNone(dc.switch_on_row(dump, "Nothing"))

    def test_failed_locks_in_the_server_s_log(self):
        esc = chr(27)
        log = chr(10).join([
            f"WARN x: phone-a left, but the lock failed: {esc}[1mthe lock file: denied",
            "WARN x: phone-b left, but the lock failed: the lock file: denied",
            "INFO x: phone-a left: locked this computer (the lock file)",
        ])
        self.assertEqual(dc.lock_failures(log, "phone-a"), 1)

    def test_every_throwaway_has_the_lock_seam(self):
        import tempfile
        with tempfile.TemporaryDirectory() as name:
            env = dc.Server(pathlib.Path(name), pathlib.Path(name) / "pocketdesk.exe").env()
            self.assertEqual(env["POCKETDESK_LOCK_FILE"], str(pathlib.Path(name) / "lock.txt"))
    def test_the_brightness_of_a_stretch_of_screen(self):
        white, black = bytes([255, 255, 255, 255]), bytes([0, 0, 0, 255])
        # 4x2: the left half white, the right half black.
        row = white * 2 + black * 2
        shot = (4, 2, row * 2)
        self.assertEqual(dc.brightness(shot, 0, 0, size=2), 255.0)
        self.assertEqual(dc.brightness(shot, 2, 0, size=2), 0.0)

    def test_screencap_s_raw_form(self):
        import struct
        pixels = bytes(range(16))  # 2x2, 4 bytes each
        self.assertEqual(dc.parse_raw(struct.pack("<III", 2, 2, 1) + pixels), (2, 2, pixels))
        self.assertEqual(dc.parse_raw(struct.pack("<IIII", 2, 2, 1, 0) + pixels), (2, 2, pixels))
        self.assertIsNone(dc.parse_raw(struct.pack("<III", 3, 3, 1) + pixels))
        self.assertIsNone(dc.parse_raw(b"short"))

    def test_a_view_s_bounds_by_its_text(self):
        dump = ('<node text="Clear" bounds="[1,2][3,4]" />'
                '<node text="Nothing copied yet. Text that crosses&#10;appears here." bounds="[10,200][590,260]" />')
        self.assertEqual(dc.node_box(dump, "Nothing copied yet"), (10, 200, 590, 260))
        self.assertIsNone(dc.node_box(dump, "Something else"))

    def test_how_much_of_a_square_is_drawn(self):
        # A 10x10 white screen with a 4x4 grey mark at (2, 2).
        width = height = 10
        pixels = bytearray([255, 255, 255, 255] * width * height)
        for row in range(2, 6):
            for col in range(2, 6):
                at = (row * width + col) * 4
                pixels[at:at + 3] = bytes([120, 120, 120])
        shot = (width, height, bytes(pixels))
        self.assertAlmostEqual(dc.marked_share(shot, (0, 0, 8, 8), (9, 9)), 16 / 64)
        self.assertEqual(dc.marked_share(shot, (6, 6, 10, 10), (9, 9)), 0.0)
        # A square off the screen is nothing drawn, not a division by zero.
        self.assertEqual(dc.marked_share(shot, (20, 20, 30, 30), (9, 9)), 0.0)

    def test_hardware_rungs_given_up_in_the_server_s_log(self):
        esc = chr(27)
        log = chr(10).join([
            f"{esc}[33mWARN{esc}[0m pocketdesk_core::streaming::encoder: Hardware encoder unavailable (Nvenc failed) (C2)",
            "INFO Video encoder: ffmpeg sidecar",
            "WARN Hardware encoder unavailable (Mf failed) (C2)",
        ])
        self.assertEqual(dc.fallbacks(log), 2)
        self.assertEqual(dc.fallbacks(""), 0)

    def test_preset_changes_in_the_server_s_log(self):
        esc = chr(27)
        log = chr(10).join([
            f"INFO {esc}[3mpreset{esc}[0m=2 Link-profile preset changed",
            "INFO Relay quality override changed",
            "INFO preset=0 Link-profile preset changed",
        ])
        self.assertEqual(dc.preset_changes(log), 2)
        self.assertEqual(dc.preset_changes(""), 0)

    def test_the_phone_s_ringer_from_dumpsys(self):
        dump = chr(10).join(["Ringer mode: ", "- mode (internal) = VIBRATE", "- mode (external) = SILENT"])
        self.assertEqual(dc.ringer_mode(dump), "silent")
        self.assertIsNone(dc.ringer_mode("nothing here"))

    def test_the_ringer_the_computer_keeps(self):
        import json, tempfile
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "state").mkdir()
            self.assertIsNone(dc.ringer_of(root, "aaa"))
            (root / "state" / "phone_status.json").write_text(
                json.dumps({"batteries": {"aaa": {"ringer": "vibrate"}, "bbb": {}}}), encoding="utf-8")
            self.assertEqual(dc.ringer_of(root, "aaa"), "vibrate")
            self.assertIsNone(dc.ringer_of(root, "bbb"))


if __name__ == "__main__":
    unittest.main()
