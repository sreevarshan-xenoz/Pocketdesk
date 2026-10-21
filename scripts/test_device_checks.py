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

    def test_every_check_names_a_checklist_section(self):
        for cid, (section, _) in dc.CHECKS.items():
            self.assertRegex(section, r"^§\d+$", cid)


if __name__ == "__main__":
    unittest.main()
