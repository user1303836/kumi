"""install.sh at the edges: releases served from a temporary folder, a home of its own for every run, and
stand-ins for the programs that say what the computer is. Checks what the installer says and that Kumi's
folder is left whole. Nothing outside the temporary folder is touched.

    python3 -m unittest discover -s scripts/tests -p test_install_script.py
"""
import functools
import hashlib
import http.server
import json
import os
import platform
import re
import shutil
import signal
import subprocess
import tarfile
import tempfile
import threading
import time
import unittest
import warnings
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INSTALL_SH = ROOT / "install.sh"
INSTALL_ONE_LINER = "curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh"

# A Kumi that says its version, and otherwise what it was started with.
FIXTURE = """#!/bin/sh
if [ "${1:-}" = "--version" ]; then echo "Kumi VERSION"; exit 0; fi
echo "fixture VERSION from=${KUMI_FROM_INSTALLER:-} home=$KUMI_HOME args=$*"
"""
BROKEN = """#!/bin/sh
echo "something went wrong first" >&2
echo "kumi: version GLIBC_2.35 not found" >&2
exit 1
"""


def target():
    machine = platform.machine()
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}[machine.lower()]
    if platform.system() == "Darwin":
        # The installer asks for the native architecture, under Rosetta too.
        if subprocess.run(["sysctl", "-n", "hw.optional.arm64"], capture_output=True, text=True).stdout.strip() == "1":
            arch = "aarch64"
        return f"{arch}-apple-darwin"
    return f"{arch}-unknown-linux-gnu"


class Server:
    """Serves a folder of releases. `slow` maps paths to files that come a little at a time, and `answers` paths to
    answers that stand in for the folder's."""

    def __init__(self, folder):
        self.slow, self.answers, self.asked = {}, {}, []
        server = self

        class Handler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                server.asked.append(self.path)
                answer = server.answers.get(self.path)
                if answer:
                    status, body, kind = answer
                    self.send_response(status)
                    self.send_header("Content-Type", kind)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                slow = server.slow.get(self.path)
                if slow:
                    data = slow.read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    try:
                        for start in range(0, len(data), 1024):
                            self.wfile.write(data[start:start + 1024])
                            self.wfile.flush()
                            time.sleep(0.2)
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                    return
                super().do_GET()

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(folder)))
        self.url = f"http://127.0.0.1:{self.httpd.server_port}"
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join()


@unittest.skipIf(os.name == "nt", "install.sh is for macOS and Linux; install.ps1 has its own checks")
class InstallScript(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="kumi-install-script-")
        self.root = Path(self.directory.name)
        self.target = target()
        self.releases = {}
        self.server = None
        self.stubs = self.root / "stubs"
        self.stubs.mkdir()

    def tearDown(self):
        if self.server:
            self.server.close()
        self.directory.cleanup()

    # ── Releases ───────────────────────────────────────────────────────────
    def release(self, version, kumi=None, sha=None, xz=False):
        """A folder holding Kumi `version` for this computer, as a release does; served from self.url. `xz`: the release
        names an xz bundle beside the gzipped one, as releases from 1.24 do."""
        folder = self.root / "releases" / version
        folder.mkdir(parents=True)
        stage = self.root / "stage" / version
        stage.mkdir(parents=True)
        program = stage / "kumi"
        program.write_text((kumi or FIXTURE).replace("VERSION", version))
        program.chmod(0o755)
        (stage / "package.json").write_text(json.dumps({"name": "kumi", "version": version, "runtime": "rust-native"}, indent=2))
        bundle = f"kumi-{self.target}.tar.gz"
        with tarfile.open(folder / bundle, "w:gz") as archive:
            for path in sorted(stage.iterdir()):
                archive.add(path, arcname=path.name)
        digest = hashlib.sha256((folder / bundle).read_bytes()).hexdigest()
        manifest = {"kumi": version, "bundle": bundle, "sha256": sha or digest, "runtime": "rust-native", "target": self.target}
        if xz:
            bundle_xz = f"kumi-{self.target}.tar.xz"
            with tarfile.open(folder / bundle_xz, "w:xz") as archive:
                for path in sorted(stage.iterdir()):
                    archive.add(path, arcname=path.name)
            manifest["bundle_xz"] = bundle_xz
            manifest["sha256_xz"] = hashlib.sha256((folder / bundle_xz).read_bytes()).hexdigest()
        (folder / f"kumi-release-{self.target}.json").write_text(json.dumps(manifest, indent=2))
        (folder / "kumi-release.json").write_text(json.dumps({"kumi": version, "targets": {self.target: manifest}}))
        return folder

    def serve(self, folder):
        if self.server:
            self.server.close()
        self.server = Server(folder)
        return self.server

    def stub(self, name, script):
        path = self.stubs / name
        path.write_text("#!/bin/sh\n" + script)
        path.chmod(0o755)

    # ── Runs ───────────────────────────────────────────────────────────────
    def environment(self, home, **extra):
        env = {
            "HOME": str(home),
            "PATH": f"{self.stubs}:/usr/bin:/bin:/usr/sbin:/sbin",
            "SHELL": "/bin/bash",
            "LANG": "en_US.UTF-8",
            "KUMI_NO_MODIFY_PATH": "1",
        }
        if self.server:
            env["KUMI_RELEASES"] = self.server.url
        env.update({key: value for key, value in extra.items() if value is not None})
        return {key: value for key, value in env.items() if value != ""}

    def install(self, home, **extra):
        return subprocess.run(["sh", str(INSTALL_SH)], env=self.environment(home, **extra), capture_output=True, text=True,
                              encoding="utf-8", timeout=120, stdin=subprocess.DEVNULL)

    def home(self, name="home"):
        home = self.root / name
        home.mkdir(exist_ok=True)
        return home

    def version(self, folder):
        program = folder / "kumi"
        if not program.exists():
            return None
        return subprocess.run([str(program), "--version"], capture_output=True, text=True).stdout.strip().removeprefix("Kumi ")

    def leftovers(self, kumi):
        return sorted(path.name for path in kumi.iterdir() if path.name.startswith(".install."))

    # ── What goes wrong ───────────────────────────────────────────────────
    def test_a_missing_release_is_named_whatever_curl_exits_with(self):
        # macOS's curl exits 56, not 22, for GitHub's 404 over HTTP/2: this curl does so for every 404.
        real = shutil.which("curl")
        self.assertIsNotNone(real)
        self.stub("curl", f"""out="$("{real}" "$@")"; status=$?
printf '%s' "$out"
case "$out" in 404*) exit 56 ;; esac
exit $status
""")
        empty = self.root / "empty"
        empty.mkdir()
        self.serve(empty)
        home = self.home()
        run = self.install(home)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn(f"Kumi couldn't be installed: there's no Kumi release for this computer ({self.target}) at {self.server.url}.", run.stderr)
        self.assertNotIn("internet", run.stderr)
        self.assertFalse((home / ".kumi").exists(), "a folder this run made goes when it fails")

    def test_nothing_answering_names_the_cause(self):
        port = self.unused_port()
        home = self.home()
        run = self.install(home, KUMI_RELEASES=f"http://127.0.0.1:{port}")
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn(f"couldn't reach 127.0.0.1:{port}: the connection failed. Check that this computer is online", run.stderr)
        self.assertIn("curl: (7)", run.stderr)
        # A user name and password never show, with or without a scheme before them, an @ in the password too.
        for proxy, shown in (("http://someone:secret@127.0.0.1:9", "http://127.0.0.1:9"), ("someone:secret@127.0.0.1:9", "127.0.0.1:9"),
                             ("http://someone:sec@ret@127.0.0.1:9", "http://127.0.0.1:9")):
            proxied = self.install(home, KUMI_RELEASES=f"http://127.0.0.1:{port}", HTTPS_PROXY=proxy)
            self.assertIn(f"Proxy settings: HTTPS_PROXY={shown}.", proxied.stderr)
            self.assertNotIn("someone", proxied.stderr)
            self.assertNotIn("ret@", proxied.stderr)

    def test_a_name_that_cant_be_looked_up_is_dns(self):
        home = self.home()
        run = self.install(home, KUMI_RELEASES="http://kumi-release.invalid")
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("couldn't reach kumi-release.invalid: its address couldn't be looked up (DNS).", run.stderr)

    def test_a_wifi_sign_in_page_in_place_of_the_release(self):
        self.serve(self.release("2.0.0"))
        page = b"<!DOCTYPE html><html><head><title>Sign in to the Wi-Fi</title></head><body>Welcome</body></html>"
        self.server.answers[f"/kumi-release-{self.target}.json"] = (200, page, "text/html")
        home = self.home()
        run = self.install(home)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("a Wi-Fi sign-in page or proxy answered instead of GitHub.", run.stderr)
        # The bundle too: a page whose checksum can't match says the same, not "try again".
        del self.server.answers[f"/kumi-release-{self.target}.json"]
        self.server.answers[f"/kumi-{self.target}.tar.gz"] = (200, page, "text/html")
        run = self.install(home)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("a Wi-Fi sign-in page or proxy answered instead of GitHub.", run.stderr)

    def test_server_errors_say_try_again_later(self):
        self.serve(self.release("2.0.0"))
        self.server.answers[f"/kumi-release-{self.target}.json"] = (503, b"busy", "text/plain")
        run = self.install(self.home())
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("had a problem (HTTP 503). Try again in a few minutes.", run.stderr)

    def test_a_tar_that_reads_xz_takes_the_smaller_bundle(self):
        # macOS's tar reads xz on its own; Linux's GNU tar with xz beside it. Both run these tests.
        home = self.home()
        self.serve(self.release("1.0.0", xz=True))
        run = self.install(home)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertEqual(self.version(home / ".kumi/app"), "1.0.0")
        bundles = [path for path in self.server.asked if path.endswith((".tar.gz", ".tar.xz"))]
        self.assertEqual(bundles, [f"/kumi-{self.target}.tar.xz"])
        # A release from before names no xz bundle: the gzipped one, as ever.
        self.serve(self.release("2.0.0"))
        self.assertEqual(self.install(home).returncode, 0)
        self.assertEqual(self.version(home / ".kumi/app"), "2.0.0")
        self.assertEqual([path for path in self.server.asked if path.endswith((".tar.gz", ".tar.xz"))], [f"/kumi-{self.target}.tar.gz"])

    def test_a_bad_checksum_leaves_the_kumi_that_was_there(self):
        home = self.home()
        self.serve(self.release("1.0.0"))
        self.assertEqual(self.install(home).returncode, 0)
        self.serve(self.release("2.0.0", sha="0" * 64))
        run = self.install(home)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("Kumi's download didn't match its checksum, so it wasn't used. Try again.", run.stderr)
        self.assertEqual(self.version(home / ".kumi/app"), "1.0.0")
        self.assertEqual(self.leftovers(home / ".kumi"), [])

    def test_a_kumi_that_doesnt_start_says_why_and_changes_nothing(self):
        home = self.home()
        self.serve(self.release("1.0.0"))
        self.assertEqual(self.install(home).returncode, 0)
        self.serve(self.release("2.0.0", kumi=BROKEN))
        run = self.install(home)
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("the downloaded Kumi didn't start (kumi: version GLIBC_2.35 not found).", run.stderr)
        self.assertEqual(self.version(home / ".kumi/app"), "1.0.0")
        self.assertFalse((home / ".kumi/app.previous").exists())

    def test_ctrl_c_changes_nothing(self):
        home = self.home()
        self.serve(self.release("1.0.0"))
        self.assertEqual(self.install(home).returncode, 0)
        folder = self.release("2.0.0")
        # A bigger download, a little at a time, so the installer is still at it when Ctrl-C comes.
        with open(folder / f"kumi-{self.target}.tar.gz", "ab") as bundle:
            bundle.write(os.urandom(64 * 1024))
        manifest = json.loads((folder / f"kumi-release-{self.target}.json").read_text())
        manifest["sha256"] = hashlib.sha256((folder / manifest["bundle"]).read_bytes()).hexdigest()
        (folder / f"kumi-release-{self.target}.json").write_text(json.dumps(manifest))
        self.serve(folder)
        self.server.slow[f"/kumi-{self.target}.tar.gz"] = folder / f"kumi-{self.target}.tar.gz"
        for when in ("download", "lookup"):
            if when == "lookup":
                self.server.slow = {f"/kumi-release-{self.target}.json": folder / f"kumi-release-{self.target}.json"}
                (folder / f"kumi-release-{self.target}.json").write_text(json.dumps(manifest) + " " * 8192)
            process = subprocess.Popen(["sh", str(INSTALL_SH)], env=self.environment(home), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       stdin=subprocess.DEVNULL, text=True, encoding="utf-8", start_new_session=True)
            wanted = f"/kumi-{self.target}.tar.gz" if when == "download" else f"/kumi-release-{self.target}.json"
            deadline = time.time() + 30
            while wanted not in self.server.asked and time.time() < deadline:
                time.sleep(0.05)
            time.sleep(0.5)
            os.killpg(process.pid, signal.SIGINT)
            out, err = process.communicate(timeout=30)
            self.server.asked.clear()
            self.assertEqual(process.returncode, 130, f"{when}: {out}{err}")
            self.assertIn("Stopped; nothing was changed.", err, when)
            self.assertNotIn("couldn't", err, when)
            self.assertEqual(self.version(home / ".kumi/app"), "1.0.0", when)
            self.assertEqual(self.leftovers(home / ".kumi"), [], when)

    def test_ctrl_c_on_a_first_install_leaves_no_folder(self):
        folder = self.release("2.0.0")
        self.serve(folder)
        self.server.slow[f"/kumi-release-{self.target}.json"] = folder / f"kumi-release-{self.target}.json"
        (folder / f"kumi-release-{self.target}.json").write_text((folder / f"kumi-release-{self.target}.json").read_text() + " " * 8192)
        home = self.home()
        process = subprocess.Popen(["sh", str(INSTALL_SH)], env=self.environment(home), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   stdin=subprocess.DEVNULL, text=True, encoding="utf-8", start_new_session=True)
        deadline = time.time() + 30
        while not self.server.asked and time.time() < deadline:
            time.sleep(0.05)
        time.sleep(0.5)
        os.killpg(process.pid, signal.SIGINT)
        out, err = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 130, out + err)
        self.assertFalse((home / ".kumi").exists())

    # ── Versions ───────────────────────────────────────────────────────────
    def test_kumi_version_is_checked_before_anything_is_downloaded(self):
        home = self.home()
        for given, said in (
            ("1.13", 'KUMI_VERSION is "1.13"; it takes a release such as 1.16.0, or latest.'),
            ("1.0.0", "Kumi 1.0.0 came before the installer (1.1.0). The releases are at https://github.com/user1303836/kumi/releases"),
            ("v1.7.5", "Kumi 1.7.5 came before native Kumi (1.7.6): install it with that release's own script: "
                       "curl -fsSL https://raw.githubusercontent.com/user1303836/kumi/v1.7.5/install.sh | KUMI_VERSION=1.7.5 sh"),
        ):
            run = self.install(home, KUMI_VERSION=given)
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn(said, run.stderr)
            self.assertFalse((home / ".kumi").exists())

    def test_versions_compare_as_numbers(self):
        home = self.home()
        self.serve(self.release("1.9.0"))
        self.assertEqual(self.install(home).returncode, 0)
        self.serve(self.release("1.10.0"))
        run = self.install(home)
        self.assertIn("1.9.0 → 1.10.0", run.stdout)
        self.serve(self.release("1.8.10"))
        run = self.install(home)
        self.assertIn("replaced 1.10.0 with older 1.8.10", run.stdout)

    # ── Repairs and rollback targets ───────────────────────────────────────
    def test_a_repair_keeps_the_kumi_to_roll_back_to(self):
        home = self.home()
        kumi = home / ".kumi"
        self.serve(self.release("1.0.0"))
        first = self.install(home)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertNotIn("reinstalled", first.stdout)
        # Fresh, then the same again: the one before is kept to roll back to, as kumi update does.
        again = self.install(home)
        self.assertIn("reinstalled 1.0.0", again.stdout)
        self.assertEqual(self.version(kumi / "app.previous"), "1.0.0")
        self.serve(self.release("2.0.0"))
        update = self.install(home)
        self.assertIn("1.0.0 → 2.0.0", update.stdout)
        self.assertEqual((self.version(kumi / "app"), self.version(kumi / "app.previous")), ("2.0.0", "1.0.0"))
        repair = self.install(home)
        self.assertEqual(repair.returncode, 0, repair.stderr)
        self.assertIn("reinstalled 2.0.0", repair.stdout)
        self.assertEqual((self.version(kumi / "app"), self.version(kumi / "app.previous")), ("2.0.0", "1.0.0"))
        self.assertEqual(self.leftovers(kumi), [])

    def test_the_only_whole_kumi_is_never_cleared(self):
        home = self.home()
        kumi = home / ".kumi"
        self.serve(self.release("1.0.0"))
        self.install(home)
        self.serve(self.release("2.0.0"))
        self.install(home)
        # An update that gave up left app empty: 2.0.0 waits in app.previous, 1.0.0 aside in app.previous.old.
        (kumi / "app.previous").rename(kumi / "app.previous.old")
        (kumi / "app").rename(kumi / "app.previous")
        (kumi / "app").mkdir()
        (kumi / "app" / "part").write_text("")
        launched = subprocess.run([str(kumi / "bin/kumi"), "--version"], capture_output=True, text=True, env=self.environment(home))
        self.assertEqual(launched.returncode, 0, launched.stderr)
        self.assertEqual(launched.stdout.strip(), "Kumi 2.0.0")
        self.assertIn("Kumi's app is missing, so the Kumi before it starts.", launched.stderr)
        self.assertIn(INSTALL_ONE_LINER, launched.stderr)
        self.serve(self.release("3.0.0"))
        run = self.install(home)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertIn("2.0.0 → 3.0.0", run.stdout)
        self.assertEqual((self.version(kumi / "app"), self.version(kumi / "app.previous")), ("3.0.0", "2.0.0"))
        self.assertFalse((kumi / "app.previous.old").exists())

    # ── The kumi command ───────────────────────────────────────────────────
    def test_the_launcher_says_what_to_do_without_an_app(self):
        home = self.home()
        kumi = home / ".kumi"
        self.serve(self.release("1.0.0"))
        self.install(home)
        shutil.rmtree(kumi / "app")
        launched = subprocess.run([str(kumi / "bin/kumi"), "--version"], capture_output=True, text=True, env=self.environment(home))
        self.assertEqual(launched.returncode, 1)
        self.assertEqual(launched.stderr, f"Kumi's app is missing: run the installer again: {INSTALL_ONE_LINER}\n")
        # Rolled back to a JavaScript Kumi, which has its own Node: that runs.
        node = kumi / "node/bin/node"
        node.parent.mkdir(parents=True)
        node.write_text('#!/bin/sh\necho "node $*"\n')
        node.chmod(0o755)
        entry = kumi / "app/apps/kumi/bin/kumi.mjs"
        entry.parent.mkdir(parents=True)
        entry.write_text("")
        launched = subprocess.run([str(kumi / "bin/kumi"), "--version"], capture_output=True, text=True, env=self.environment(home))
        self.assertEqual(launched.stdout.strip(), f"node {entry} --version")

    def test_the_launcher_is_the_one_kumi_writes(self):
        script = INSTALL_SH.read_text(encoding="utf-8")
        launcher = script.split("<<'LAUNCHER'\n", 1)[1].split("\nLAUNCHER\n", 1)[0] + "\n"
        source = (ROOT / "crates/kumi/src/install/migration.rs").read_text(encoding="utf-8")
        rust = re.search(r'^const UNIX_LAUNCHER: &str =\s*"(.*?)";$', source, re.S | re.M)[1]
        self.assertEqual(launcher, json.loads('"' + rust + '"'))

    # ── PATH ───────────────────────────────────────────────────────────────
    def test_path_left_alone_says_how_to_start_kumi(self):
        home = self.home()
        self.serve(self.release("1.0.0"))
        run = self.install(home)
        self.assertIn("left as it is (KUMI_NO_MODIFY_PATH)", run.stdout)
        self.assertIn("Next: add ~/.kumi/bin to PATH, or run ~/.kumi/bin/kumi\n", run.stdout)
        self.assertNotIn("new terminal window", run.stdout)

    def test_a_relative_kumi_home_is_made_absolute(self):
        home = self.home()
        self.serve(self.release("1.0.0"))
        run = subprocess.run(["sh", str(INSTALL_SH)], env=self.environment(home, KUMI_HOME="relkumi", KUMI_NO_MODIFY_PATH=""),
                             capture_output=True, text=True, encoding="utf-8", cwd=home, stdin=subprocess.DEVNULL)
        self.assertEqual(run.returncode, 0, run.stderr)
        # The folder it ran in, as the shell sees it (macOS's /var is /private/var).
        here = home.resolve()
        self.assertIn(f'export PATH="{here}/relkumi/bin:$PATH"', (home / ".profile").read_text())
        self.assertIn(f'(or after: export PATH="{here}/relkumi/bin:$PATH"): kumi', run.stdout)

    def test_a_kumi_home_that_is_the_home_folder_or_the_root_is_refused(self):
        # kumi uninstall takes Kumi's names (bin, downloads, tools…) out of its folder: there they'd be the producer's.
        home = self.home()
        self.serve(self.release("1.0.0"))
        link = self.root / "link to home"
        link.symlink_to(home)
        # A link into a folder of the home folder, then ..: the home folder, though not as written.
        (home / "sub").mkdir()
        into = self.root / "link into home"
        into.symlink_to(home / "sub")
        spellings = [str(home), f"{home}/", f"{home}/.", f"{home}/new/..", "~", str(link), f"{into}/.."]
        # Other case on a disk that ignores it, and macOS's own path to the data volume.
        if Path(str(home).swapcase()).exists():
            spellings.append(str(home).swapcase())
        if Path(f"/System/Volumes/Data{home.resolve()}").exists():
            spellings.append(f"/System/Volumes/Data{home.resolve()}")
        for kumi_home in spellings:
            run = self.install(home, KUMI_HOME=kumi_home)
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn("KUMI_HOME is your home folder; Kumi needs a folder of its own, such as ~/.kumi.", run.stderr)
            self.assertEqual([path.name for path in home.iterdir()], ["sub"], kumi_home)
        # As root, a refusal that failed would install into / or /dev: those runs are left to everyone else.
        if os.geteuid() != 0:
            run = self.install(home, KUMI_HOME="/")
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn("KUMI_HOME is /, the root of the disk; Kumi needs a folder of its own", run.stderr)
            # Where a disk is mounted, as kumi uninstall would refuse it: /dev is one on macOS and Linux.
            run = self.install(home, KUMI_HOME="/dev")
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn("KUMI_HOME is /dev, where a disk is mounted; Kumi needs a folder of its own there, such as /dev/kumi.", run.stderr)
            # Through a link to it, too.
            mounted = self.root / "to dev"
            mounted.symlink_to("/dev")
            run = self.install(home, KUMI_HOME=str(mounted))
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn(", where a disk is mounted; Kumi needs a folder of its own there", run.stderr)

    @unittest.skipUnless(shutil.which("zsh"), "no zsh here")
    def test_run_by_zsh_it_reads_paths_as_sh_does(self):
        # curl … | zsh: zsh splits no words unless told to, which reading . and .. counts on.
        home = self.home()
        self.serve(self.release("1.0.0"))
        cases = [(f"{home}/new/..", "KUMI_HOME is your home folder")]
        # As root, a refusal that failed would install into /private or /: that run is left to everyone else.
        if os.geteuid() != 0:
            cases.append(("/tmp/..", "KUMI_HOME is /, the root of the disk"))
        for kumi_home, said in cases:
            run = subprocess.run(["zsh"], input=INSTALL_SH.read_text(), env=self.environment(home, KUMI_HOME=kumi_home),
                                 capture_output=True, text=True, encoding="utf-8", timeout=120)
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn(said, run.stderr)
            self.assertEqual(list(home.iterdir()), [], kumi_home)

    def test_the_zsh_line_goes_where_zsh_reads_it_once(self):
        home = self.home()
        zdot = home / "zsh config"
        self.serve(self.release("1.0.0"))
        for _ in range(2):
            run = self.install(home, SHELL="/bin/zsh", ZDOTDIR=str(zdot), KUMI_NO_MODIFY_PATH="")
            self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual((zdot / ".zshrc").read_text().count("# Added by the Kumi installer"), 1)
        self.assertFalse((home / ".zshrc").exists())

    def test_an_edited_path_line_still_counts(self):
        # Someone spelled the line from their home folder (for dotfiles they sync): no second block. A line for a Kumi
        # elsewhere isn't this one's, so this one gets its own.
        self.serve(self.release("1.0.0"))
        for spelled in ('export PATH="$HOME/.kumi/bin:$PATH"', "export PATH=${HOME}/.kumi/bin:$PATH", "path+=(~/.kumi/bin)",
                        'export PATH="/elsewhere/.kumi/bin:$PATH"'):
            home = self.home(f"edited {len(spelled)}")
            (home / ".zshrc").write_text(f"# mine\n# Added by the Kumi installer\n{spelled}\n")
            run = self.install(home, SHELL="/bin/zsh", KUMI_NO_MODIFY_PATH="")
            self.assertEqual(run.returncode, 0, run.stderr)
            elsewhere = "elsewhere" in spelled
            self.assertEqual((home / ".zshrc").read_text().count("# Added by the Kumi installer"), 2 if elsewhere else 1, spelled)
            self.assertIn(f'export PATH="{home}/.kumi/bin:$PATH"' if elsewhere else "PATH                done\n", (home / ".zshrc").read_text() if elsewhere else run.stdout)

    def test_a_home_ending_in_a_slash_gets_one_slash_and_one_block(self):
        # With HOME=/home/me/, the line was …//.kumi/bin, which kumi uninstall didn't know as Kumi's. An earlier
        # installer's line, written so, still counts: no second block.
        self.serve(self.release("1.0.0"))
        home = self.home("slash")
        for _ in range(2):
            run = self.install(home, HOME=f"{home}/", SHELL="/bin/zsh", KUMI_NO_MODIFY_PATH="")
            self.assertEqual(run.returncode, 0, run.stderr)
        text = (home / ".zshrc").read_text()
        self.assertEqual(text.count("# Added by the Kumi installer"), 1, text)
        self.assertIn(f'export PATH="{home}/.kumi/bin:$PATH"', text)
        self.assertNotIn("//", text)
        earlier = self.home("earlier slash")
        (earlier / ".zshrc").write_text(f'# Added by the Kumi installer\nexport PATH="{earlier}//.kumi/bin:$PATH"\n')
        run = self.install(earlier, HOME=f"{earlier}/", SHELL="/bin/zsh", KUMI_NO_MODIFY_PATH="")
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual((earlier / ".zshrc").read_text().count("# Added by the Kumi installer"), 1)

    # ── This computer ──────────────────────────────────────────────────────
    def test_sudo_is_told_kumi_would_install_for_root(self):
        self.stub("id", 'if [ "$1" = "-u" ]; then echo 0; else exec /usr/bin/id "$@"; fi\n')
        self.serve(self.release("1.0.0"))
        home = self.home()
        run = self.install(home, SUDO_USER="someone")
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("it's running with sudo, so Kumi would be installed for root, not for someone.", run.stderr)
        self.assertIn("KUMI_ALLOW_ROOT=1", run.stderr)
        self.assertEqual(self.install(home, SUDO_USER="someone", KUMI_ALLOW_ROOT="1").returncode, 0)
        # Root itself (a container, a root-only machine), and sudo by root, install as before.
        for sudo in (None, "root"):
            self.assertEqual(self.install(self.home(f"root {sudo}"), SUDO_USER=sudo).returncode, 0, sudo)

    def test_with_wget_its_own_exit_statuses_are_read(self):
        # A computer with wget and no curl: every program here but curl, and a wget that fails as asked, or gets the
        # file with curl, saying the status as wget -S does.
        tools = self.root / "tools"
        tools.mkdir()
        for folder in ("/usr/bin", "/bin", "/usr/sbin", "/sbin"):
            for program in Path(folder).iterdir():
                if program.name not in {"curl", "wget"} and not (tools / program.name).exists() and os.access(program, os.X_OK):
                    (tools / program.name).symlink_to(program)
        curl = shutil.which("curl")
        self.stub("wget", f"""for argument; do file="$url"; url="$argument"; done
case "${{FAKE_WGET:-}}" in
  tls) echo "ERROR: cannot verify 127.0.0.1's certificate" >&2; exit 5 ;;
  disk) echo "Cannot write to '$file' (No space left on device)." >&2; exit 3 ;;
  proxy) echo "Proxy tunneling failed: Proxy Authentication RequiredUnable to establish SSL connection." >&2; exit 6 ;;
  dns) echo "wget: unable to resolve host address 'kumi.invalid'" >&2; exit 4 ;;
esac
status="$("{curl}" -s -o "$file" -w '%{{http_code}}' "$url")"
echo "  HTTP/1.1 $status Whatever" >&2
[ "$status" = 200 ] || exit 8
""")
        self.serve(self.release("1.0.0"))
        home = self.home()
        path = f"{self.stubs}:{tools}"
        self.assertEqual(self.install(home, PATH=path).returncode, 0)
        for fake, said in (("tls", "the secure connection didn't check out (TLS)"), ("disk", "it couldn't save the download in"),
                           ("proxy", "the proxy didn't let it through"), ("dns", "its address couldn't be looked up (DNS)")):
            run = self.install(home, PATH=path, FAKE_WGET=fake)
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn(said, run.stderr, fake)
        empty = self.root / "empty"
        empty.mkdir()
        self.serve(empty)
        run = self.install(home, PATH=path)
        self.assertIn(f"there's no Kumi release for this computer ({self.target})", run.stderr)

    @unittest.skipUnless(platform.system() == "Darwin", "Rosetta is a Mac's")
    def test_a_shell_under_rosetta_gets_the_native_build(self):
        self.stub("uname", 'case "$1" in -m) echo x86_64 ;; *) exec /usr/bin/uname "$@" ;; esac\n')
        self.stub("sysctl", 'case "$*" in *proc_translated*) echo 1 ;; *hw.optional.arm64*) exit 1 ;; *) exec /usr/sbin/sysctl "$@" ;; esac\n')
        self.target = "aarch64-apple-darwin"
        self.serve(self.release("1.0.0"))
        run = self.install(self.home())
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertIn("/kumi-release-aarch64-apple-darwin.json", self.server.asked)

    @unittest.skipUnless(platform.system() == "Linux", "glibc is Linux's")
    def test_an_older_glibc_is_named(self):
        self.stub("getconf", 'if [ "$1" = "GNU_LIBC_VERSION" ]; then echo "glibc 2.31"; else exec /usr/bin/getconf "$@"; fi\n')
        self.serve(self.release("1.0.0"))
        run = self.install(self.home())
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn("Kumi needs glibc 2.35 or later (Ubuntu 22.04, Debian 12 or newer); this Linux has glibc 2.31.", run.stderr)

    # ── A terminal ─────────────────────────────────────────────────────────
    def terminal(self, home, **extra):
        """install.sh in a terminal of its own: its exit status, and what it wrote with the colours taken out."""
        import pty
        env = self.environment(home, TERM="xterm-256color", COLUMNS="80", **extra)
        with warnings.catch_warnings():
            # The server's thread: the child only runs exec.
            warnings.simplefilter("ignore", DeprecationWarning)
            pid, fd = pty.fork()
        if pid == 0:
            os.execve("/bin/sh", ["sh", str(INSTALL_SH)], env)
        output = b""
        while True:
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                break
            if not chunk:
                break
            output += chunk
        _, status = os.waitpid(pid, 0)
        os.close(fd)
        return os.waitstatus_to_exitcode(status), re.sub(r"\x1b\[[0-9;]*m", "", output.decode("utf-8", "replace"))

    def test_in_a_terminal_a_checksum_that_doesnt_match_follows_the_download_line(self):
        self.serve(self.release("1.0.0", sha="0" * 64))
        status, text = self.terminal(self.home(), KUMI_NO_LAUNCH="1")
        self.assertEqual(status, 1, text)
        # Curl's bar goes, the step's line comes back, and the error follows it rather than taking its place.
        self.assertRegex(text, r"\x1b\[2A\r\x1b\[J  Download +…\r?\n\r?\nKumi couldn't be installed: Kumi's download didn't match")

    def test_in_a_terminal_kumi_starts_and_knows_this_window_has_no_kumi_yet(self):
        home = self.home()
        self.serve(self.release("1.0.0"))
        status, text = self.terminal(home, KUMI_NO_MODIFY_PATH="")
        self.assertEqual(status, 0, text)
        self.assertIn("Looking up the newest Kumi…", text)
        self.assertIn("Starting Kumi…", text)
        self.assertIn(f"fixture 1.0.0 from=new-window home={home}/.kumi", text)
        self.assertEqual(self.leftovers(home / ".kumi"), [])

    @staticmethod
    def unused_port():
        import socket
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            return probe.getsockname()[1]


if __name__ == "__main__":
    unittest.main()
