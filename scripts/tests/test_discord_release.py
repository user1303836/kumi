"""The Discord post of a release's notes: what's taken out, how long notes are cut, and what a message can't do."""
import importlib.util
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("discord_release", Path(__file__).parents[1] / "discord_release.py")
discord = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(discord)


class DiscordReleaseNotes(unittest.TestCase):
    def test_release_scripts_marker_and_the_space_around_it_are_taken_out(self):
        body = "Kumi 1.24.30. Ships with bridge **1.0.101**.\n\n- One thing.\n\n<!-- kumi:release-notes -->\n"
        self.assertEqual(discord.notes(body), "Kumi 1.24.30. Ships with bridge **1.0.101**.\n\n- One thing.")

    def test_short_notes_are_one_message_titled_with_the_release_and_linking_to_it(self):
        sent = discord.messages("Kumi 1.24.30", "https://github.com/user1303836/kumi/releases/tag/v1.24.30", "- One thing.")
        self.assertEqual(len(sent), 1)
        self.assertEqual(sent[0]["embeds"], [{
            "title": "Kumi 1.24.30",
            "url": "https://github.com/user1303836/kumi/releases/tag/v1.24.30",
            "description": "- One thing.",
        }])

    def test_long_notes_go_on_in_more_messages_cut_between_paragraphs(self):
        paragraphs = [f"- Change {n}: " + "words " * 60 for n in range(40)]
        text = "\n\n".join(paragraphs)
        sent = discord.messages("Kumi 1.24.30", "https://example.com", text)
        self.assertGreater(len(sent), 1)
        self.assertIn("title", sent[0]["embeds"][0])
        self.assertTrue(all("title" not in message["embeds"][0] for message in sent[1:]))
        descriptions = [message["embeds"][0]["description"] for message in sent]
        self.assertTrue(all(len(d) <= discord.LIMIT for d in descriptions))
        self.assertEqual("\n\n".join(descriptions), text)

    def test_a_paragraph_longer_than_a_message_is_cut_between_its_lines(self):
        paragraph = "\n".join(f"- Line {n}: " + "x" * 90 for n in range(100))
        parts = discord.pieces(paragraph)
        self.assertGreater(len(parts), 1)
        self.assertTrue(all(len(part) <= discord.LIMIT for part in parts))
        self.assertEqual("\n".join(parts), paragraph)

    def test_a_line_longer_than_a_message_is_cut_where_it_has_to_be(self):
        line = "y" * (discord.LIMIT * 2 + 10)
        parts = discord.pieces(line)
        self.assertEqual([len(part) for part in parts], [discord.LIMIT, discord.LIMIT, 10])
        self.assertEqual("".join(parts), line)

    def test_no_message_can_mention_anyone_whatever_the_notes_say(self):
        sent = discord.messages("Kumi 1.24.30", "https://example.com", "@everyone and <@123> and <@&456>")
        self.assertTrue(all(message["allowed_mentions"] == {"parse": []} for message in sent))

    def test_empty_notes_still_post_the_release_title_and_link(self):
        self.assertEqual(discord.messages("Kumi 1.24.30", "https://example.com", ""),
                         [{"embeds": [{"title": "Kumi 1.24.30", "url": "https://example.com"}],
                           "allowed_mentions": {"parse": []}}])


if __name__ == "__main__":
    unittest.main()
