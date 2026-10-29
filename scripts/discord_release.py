#!/usr/bin/env python3
"""Post a published Kumi release's notes to Discord's announcement channel.

    python3 scripts/discord_release.py v1.24.30            # posts through the webhook in DISCORD_RELEASES_WEBHOOK
    python3 scripts/discord_release.py v1.24.30 --dry-run  # prints the messages instead

The tag's Installer run calls it once it has published the release, every install having passed; the "Release notes
to Discord" workflow calls it by hand (a release published before the webhook, or a post that failed). It reads the
release with gh, so gh has to reach the repository (GH_TOKEN in a workflow). A draft is never posted, and without a
webhook it says so and posts nothing.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
import urllib.error
import urllib.request

MARKER = "<!-- kumi:release-notes -->"
# An embed's description holds 4,096 characters and a message's embeds 6,000 between them: one embed a message, each
# kept under the first with room to spare.
LIMIT = 4000
# Discord turns away urllib's own User-Agent.
USER_AGENT = "kumi-release-notes (https://github.com/user1303836/kumi, 1)"


def notes(body: str) -> str:
    """The notes as producers read them: release.py's marker taken out."""
    return body.replace(MARKER, "").strip()


def pieces(text: str, limit: int = LIMIT) -> list[str]:
    """`text` in pieces of at most `limit` characters, cut between paragraphs where it can, else between lines, else
    within a line."""
    out: list[str] = []
    current = ""

    def add(part: str, joint: str) -> bool:
        nonlocal current
        joined = f"{current}{joint}{part}" if current else part
        if len(joined) > limit:
            return False
        current = joined
        return True

    def close() -> None:
        nonlocal current
        if current:
            out.append(current)
        current = ""

    for paragraph in text.split("\n\n"):
        if add(paragraph, "\n\n"):
            continue
        close()
        if add(paragraph, "\n\n"):
            continue
        for line in paragraph.split("\n"):
            if add(line, "\n"):
                continue
            close()
            while len(line) > limit:
                out.append(line[:limit])
                line = line[limit:]
            current = line
    close()
    return out


def messages(name: str, url: str, text: str) -> list[dict]:
    """The webhook messages for a release: the first titled with its name and linking to it, the rest going on. None
    can mention anyone, whatever the notes say."""
    out = []
    for index, part in enumerate(pieces(text) or [""]):
        embed: dict = {"title": name, "url": url} if index == 0 else {}
        if part:
            embed["description"] = part
        out.append({"embeds": [embed], "allowed_mentions": {"parse": []}})
    return out


def post(webhook: str, message: dict) -> None:
    """Send one message, waiting out Discord's rate limit when it asks."""
    joint = "&" if "?" in webhook else "?"
    request = urllib.request.Request(
        f"{webhook}{joint}wait=true",
        data=json.dumps(message).encode(),
        method="POST",
        headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
    )
    for attempt in range(5):
        try:
            with urllib.request.urlopen(request, timeout=30):
                return
        except urllib.error.HTTPError as error:
            said = error.read()
            if error.code == 429 and attempt < 4:
                time.sleep(float(json.loads(said or b"{}").get("retry_after", 1)) + 0.5)
                continue
            raise SystemExit(f"Discord turned the message down: HTTP {error.code} {said[:300]!r}") from None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("tag", help="the release's tag, such as v1.24.30")
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"), help="owner/name (gh's own by default)")
    parser.add_argument("--dry-run", action="store_true", help="print the messages instead of posting them")
    args = parser.parse_args()
    command = ["gh", "release", "view", args.tag, "--json", "name,body,url,isDraft"]
    if args.repo:
        command += ["--repo", args.repo]
    read = subprocess.run(command, capture_output=True, text=True)
    if read.returncode:
        raise SystemExit(f"gh couldn't read {args.tag}: {read.stderr.strip()}")
    release = json.loads(read.stdout)
    if release["isDraft"]:
        raise SystemExit(f"{args.tag} is still a draft: only a published release is posted.")
    sent = messages(release["name"] or f"Kumi {args.tag.removeprefix('v')}", release["url"], notes(release["body"]))
    if args.dry_run:
        for message in sent:
            print(json.dumps(message, ensure_ascii=False, indent=2))
        return
    webhook = os.environ.get("DISCORD_RELEASES_WEBHOOK", "").strip()
    if not webhook:
        print("DISCORD_RELEASES_WEBHOOK is empty, so nothing is posted.")
        return
    for message in sent:
        post(webhook, message)
    print(f"Posted {args.tag}'s notes to Discord in {len(sent)} message(s).")


if __name__ == "__main__":
    main()
