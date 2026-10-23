"""Operator-only Live fixture test; never run against a production Set.

Call run(provider, mapper_module, song) on Live's main thread with Kumi's
_WillingtonProvider and its mapper module. The provider must already have loaded
WillingtonEditing with writes disabled. Use a disposable stopped Set named
'Willington Native Editing', two ungrouped first tracks, and a MIDI clip in track
1 / slot 0 containing note ID 1. A passing receipt is written beside the loaded
package, bound to the selected library. Failure invalidates the old receipt.
This macOS fixture runner is not a model tool or part of bridge startup.
"""
import hashlib
import json
import os
from pathlib import Path
import tempfile


def run(provider, module, song):
    import WillingtonEditing
    if os.name != "posix":
        raise RuntimeError("this fixture runner requires POSIX owner-only files")
    if song.name != "Willington Native Editing" or song.is_playing:
        raise RuntimeError("open the stopped disposable Willington Native Editing Set")
    native = provider.editing
    if native is None or native.writes_enabled:
        raise RuntimeError("load the native editing provider with writes disabled first")
    mapper = provider.mapper
    receipt = Path(WillingtonEditing.__file__).with_name("self-test.json")
    library = Path(native.path)
    digest = hashlib.sha256(library.read_bytes()).hexdigest()

    def save(status, checks):
        data = {"component": "WillingtonEditing", "status": status,
                "library_sha256": digest, "checks": checks}
        fd, temporary = tempfile.mkstemp(prefix=".self-test-", dir=str(receipt.parent))
        try:
            with os.fdopen(fd, "w") as stream:
                json.dump(data, stream, sort_keys=True)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, receipt)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
        return data

    save("running", [])
    checks = []
    before_tracks = list(song.tracks)
    if len(before_tracks) < 2 or any(t.group_track is not None for t in before_tracks[:2]):
        raise RuntimeError("first two fixture tracks must be ungrouped")
    track = song.tracks[1]
    clip = track.clip_slots[0].clip
    if clip is None or not clip.is_midi_clip:
        raise RuntimeError("fixture track 1 / slot 0 must contain a MIDI clip with note ID 1")
    scene = song.scenes[0]
    parameter = track.mixer_device.volume
    track_ref = mapper.refs.put("track", track, "1")
    clip_ref = mapper.refs.put("clip", clip, "1:0")
    scene_ref = mapper.refs.put("scene", scene, "0")
    parameter_ref = mapper.refs.put("parameter", parameter, "mixer:1:volume")
    cases = [
        ({"kind": "global-follow"}, {"enabled": not song.get_follow_actions_enabled()}),
        ({"kind": "scene-follow", "ref": scene_ref}, {"chance_a": 35, "loop_count": 2, "time": 1/3}),
        ({"kind": "note-expression", "ref": clip_ref, "noteId": 1, "dimension": "pressure"},
         {"exists": True, "events": [[.1, 47.25, .2, .3, .7, .8], [1.1, 92.125, .5, .5, .5, .5]]}),
        ({"kind": "arrangement-automation", "ref": track_ref, "targetRef": parameter_ref},
         {"action": "insert", "event": [60, .55, .5, .5, .5, .5]}),
    ]
    linked = json.loads(scene.get_follow_actions())["linked"]
    cases.extend([
        ({"kind": "scene-follow", "ref": scene_ref}, {"linked": not linked}),
        ({"kind": "scene-follow", "ref": scene_ref}, {"linked": True, "loop_count": 3}),
    ])
    try:
        native.enable(True)
        mapper.willington_editing_writes = True
        for selector, edit in cases:
            before = mapper.invoke("willington.editing.read", {**selector, "edit": edit})
            module.validate_operation_payload("willington.editing.read", "result", before)
            value = json.loads(before["state"])["value"]
            try:
                after = mapper.invoke("willington.editing.set", {**selector, "next": before["next"],
                    "expectedStateRevision": before["stateRevision"]})
                module.validate_operation_payload("willington.editing.set", "result", after)
                assert after["changed"] and after["stateRevision"] != before["stateRevision"]
                restored = mapper.invoke("willington.editing.set", {**selector,
                    "next": json.dumps({"restore": before["state"]}),
                    "expectedStateRevision": after["stateRevision"]})
                assert restored["stateRevision"] == before["stateRevision"]
            finally:
                kind = selector["kind"]
                if kind == "global-follow":
                    song.set_follow_actions_enabled(value)
                elif kind == "scene-follow":
                    for field, item in value.items():
                        scene.set_follow_action(field, item)
                elif kind == "note-expression":
                    clip.replace_note_expression(1, "pressure", json.dumps(value))
                else:
                    track.restore_arrangement_snapshot(parameter, json.dumps(value))
            current = mapper.invoke("willington.editing.read", selector)
            assert current["stateRevision"] == before["stateRevision"]
            checks.append({"kind": kind, "edit": edit, "observed": json.loads(after["state"])["value"] if kind != "arrangement-automation" else {"snapshotCaptured": True}, "applied": True, "restoredExact": True})
        selected = before_tracks[:2]
        selector = {"kind": "group-tracks", "trackRefs": [
            mapper.refs.put("track", t, str(i)) for i, t in enumerate(selected)]}
        try:
            before = mapper.invoke("willington.editing.read", {**selector, "edit": {"action": "group"}})
            after = mapper.invoke("willington.editing.set", {**selector, "next": before["next"],
                "expectedStateRevision": before["stateRevision"]})
            module.validate_operation_payload("willington.editing.set", "result", after)
            assert after["changed"] and json.loads(after["summary"])["createdRef"]
            assert selected[0].group_track is not None and selected[0].group_track == selected[1].group_track
        finally:
            created = selected[0].group_track
            if created is not None:
                song.ungroup_track(created)
        assert list(song.tracks) == before_tracks
        checks.append({"kind": "group-tracks", "applied": True, "topologyRestored": True,
                       "cleanup": "explicit native ungroup; no Kumi history inverse"})
        assert not song.is_playing
        assert hashlib.sha256(library.read_bytes()).hexdigest() == digest
    finally:
        mapper.willington_editing_writes = False
        native.enable(False)
    assert not native.writes_enabled
    return save("passed", checks)
