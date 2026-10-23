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
            self.assertRegex(section, r"^§\d+$", cid)


if __name__ == "__main__":
    unittest.main()
