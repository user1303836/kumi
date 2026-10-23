# Logic Pro M8: the release pipeline, the docs, and the last acceptance run, 2026-10-21

Status: the release pipeline's checks and Kumi's tests pass; the last acceptance run on the real Logic Pro did not.
It passed 80 of 91 in a cascade that every earlier gate had passed, recorded below and left open. The latest full
run that passed is the M6 gate's, 95 of 95 at c6a3d631 (`logic-m6-gate-2026-10-21.md`). Since then the code changed in
Kumi's words and the manual's answer (M7), in how a Kumi decides it is a release and what it says without Logic's
bridge (M8), and in the release scripts, tests and docs.

Scope: `feat/logic` at 4d039f54.

## The release pipeline (checked on this Mac, not in a release)

- `scripts/build-native-release.py` stages `daws/logic/logic-kumi/` for `aarch64-apple-darwin` only: the bridge,
  exactly the helper of the current Swift source, a `version` file; the Intel Mac, Linux and Windows bundles carry none.
  The pipeline lane staged it on this Mac with the real helper and a debug bridge, and `lipo`, `codesign --verify
  --strict` and `--version` passed on what it staged. It also ran the installer job's setup, update, two rollbacks and
  uninstall by hand on a release layout, and the refusal of a Kumi that carries no Logic bridge.
- The script tests (`scripts/tests`, Python 3.14): 103 run, all passing, 5 skipped (they need CI's built releases).
  The release tests leave this checkout's real ships-with line out of their copies of the docs (0d4b6f93).
- What only CI or a real release can check: the bundles built by `ci-release` and `release` (a stripped `logic-kumi`
  passing codesign and `dyld_info`, the helper built on macos-15), the install jobs end to end (Windows' PowerShell
  lines among them), the Intel Mac and update-from-the-previous-release jobs (extended runs only), the six targets'
  aggregate, `logic-kumi.exe` linking on both Windows targets, and a real `release.py` run on main.

## Kumi's tests

The five crates' full run at 4d039f54: 2,554 passed, 5 failed, 6 ignored; each failure passes alone (the app's Logic
session, 8 s alone, and the install's startup switch, under the run's load) or is a known flaky test (two of the
helper transport's, MusicBrainz's pacing).

## The last acceptance run

The run (`accept_logic`, the M2 to M6 checks) started from `Scratch.logicx` on its full-size main window and passed
every M2, M3 and M4 check. Then, in M5's clean-up, Kumi's undo of the Gain it had put on the Stereo Out was refused
("Logic Pro's slot answered the press without keeping its menu up"), the audition heard nothing, and the Stereo Out
kept the Gain. In M6, the sample load's import panel didn't come up in time; the bridge's Escape didn't clear what
Logic then showed, and an "Open File" dialog stayed up, so every read after it was refused as Logic being busy (the
recent projects, middle C, the bulk snapshot, watch_me, the curves, the library's recent projects, the project as
before).

Each of these passed in the M4 second slice's, M5's and M6's gates on the same code paths. What differs is Logic's
state after a day of automation: before the run its main window had been closed with Scratch still open (Window >
Open Main Window brought it back), and its Window menu lists dozens of entries named after a track (Vocal) whose
plug-in window the bridge opened and closed many times. Open, in order: whether those entries are plug-in windows
Logic keeps after the bridge's close (a leak that would grow in a long session), a run after Logic is restarted, and
the bridge clearing whatever dialog Logic shows when a panel it asked for doesn't come.

## The docs

Kumi's docs say how to use Kumi with Logic Pro in English, Japanese and Chinese (the guide, the support and capability
matrices, delivery, the developer guide, testing, the TUI's page), the Japanese and Chinese docs gain the Logic
contract and the status doc's Logic entries, the READMEs carry Logic Pro 12 and Apple's trademark, and the status doc's
evidence table lists every Logic record. Logic's ships-with line names Kumi 1.22.0 (it named 1.20.0, then 1.21.0, until
each went out without Logic): the release that takes this is cut with `scripts/release.py --minor`.
