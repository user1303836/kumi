"""Explicit end-to-end fixture, called on Live's main thread in a disposable Set.

start(song, package_path, endpoint_path) loads a staged AbletonMcpBridge package
containing the verified official Willington bundle and a passing local receipt.
The package's owner-only willington.json must request editing writes. Run the
ignored crates/kumi/tests/it/native_editing_live.rs test with the returned
endpoint file, then ALWAYS call stop(song), including after a test failure.
Neither this helper nor the Rust fixture runs automatically or ships in the
bridge package.
"""
import builtins
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import sys


def start(song, package_path, endpoint_path):
    import Live
    if song.name != 'Willington Native Editing' or song.is_playing:
        raise RuntimeError('open the stopped disposable Willington Native Editing Set')
    if hasattr(Live, '_kumi_editing_e2e'):
        raise RuntimeError('stop the existing fixture first')
    endpoint_path = Path(endpoint_path)
    if endpoint_path.exists() or endpoint_path.is_symlink():
        raise RuntimeError('endpoint file must not already exist')
    base = Path(package_path)
    surface = next(s for s in builtins.control_surfaces if hasattr(s, '_bridge'))
    previous = getattr(Live, '_kumi_willington_owner', None)
    server = provider = None
    if previous:
        previous.close()
    try:
        for key in list(sys.modules):
            if key == 'kumi_live_fixture' or key.startswith('kumi_live_fixture.'):
                del sys.modules[key]
        spec = importlib.util.spec_from_file_location('kumi_live_fixture', base / '__init__.py', submodule_search_locations=[str(base)])
        package = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = package
        spec.loader.exec_module(package)
        module = sys.modules['kumi_live_fixture.ableton_mcp_remote_script']
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            port = probe.getsockname()[1]
        config = {'host': '127.0.0.1', 'port': port, 'secret': secrets.token_hex(32)}
        server = module.AbletonMcpBridge(None, config, song=song, provenance='real-live')
        logs = []
        provider = package._WillingtonProvider(server.mapper, logs.append)
        if provider.editing is None or not provider.editing.writes_enabled:
            raise RuntimeError('fixture needs a validated editing provider and passing receipt: ' + '; '.join(logs))
        mapper = server.mapper
        track = song.tracks[1]
        clip = track.clip_slots[0].clip
        scene = song.scenes[0]
        parameter = track.mixer_device.volume
        refs = {'track': mapper.refs.put('track', track, '1'),
                'clip': mapper.refs.put('clip', clip, '1:0'),
                'scene': mapper.refs.put('scene', scene, '0'),
                'parameter': mapper.refs.put('parameter', parameter, 'mixer:1:volume'),
                'groupTracks': [mapper.refs.put('track', t, str(i)) for i, t in enumerate(song.tracks[:2])]}
        selectors = [{'kind': 'global-follow'}, {'kind': 'scene-follow', 'ref': refs['scene']},
                     {'kind': 'note-expression', 'ref': refs['clip'], 'noteId': 1, 'dimension': 'pressure'},
                     {'kind': 'arrangement-automation', 'ref': refs['track'], 'targetRef': refs['parameter']}]
        prior = [mapper.invoke('willington.editing.read', s)['state'] for s in selectors]
        original = surface._bridge.update_display
        state = {'server': server, 'provider': provider, 'previous': previous,
                 'tracks': list(song.tracks), 'prior': prior, 'objects': (scene, clip, track, parameter),
                 'old_bridge': surface._bridge, 'original_update': original, 'endpoint_path': endpoint_path}
        config.update(refs=refs, enabled=song.get_follow_actions_enabled(), linked=json.loads(scene.get_follow_actions())['linked'])
        fd = os.open(str(endpoint_path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, 'w') as stream:
            json.dump(config, stream)
        # The installed surface does not pump Framework.schedule_message tasks.
        # Its display hook keeps both authenticated bridges on Live's main thread.
        def pump_both():
            server.update_display()
            original()
        Live._kumi_editing_e2e = state
        surface._bridge.update_display = pump_both
        return {'ready': True, 'library_sha256': hashlib.sha256(Path(provider.editing.path).read_bytes()).hexdigest()}
    except BaseException:
        try:
            if provider:
                provider.close()
            if server:
                server.disconnect()
        finally:
            if previous:
                type(previous).__init__(previous, previous.mapper, None)
        raise


def stop(song):
    import Live
    state = Live._kumi_editing_e2e
    state['old_bridge'].update_display = state['original_update']
    provider = state['provider']
    scene, clip, track, parameter = state['objects']
    try:
        provider.editing.enable(True)
        original = state['tracks']
        if original[0].group_track is not None and original[0].group_track not in original:
            song.ungroup_track(original[0].group_track)
        values = [json.loads(raw)['value'] for raw in state['prior']]
        song.set_follow_actions_enabled(values[0])
        for field, value in values[1].items():
            scene.set_follow_action(field, value)
        clip.replace_note_expression(1, 'pressure', json.dumps(values[2]))
        track.restore_arrangement_snapshot(parameter, json.dumps(values[3]))
        assert list(song.tracks) == original
        assert song.get_follow_actions_enabled() == values[0]
        assert json.loads(scene.get_follow_actions()) == values[1]
        assert json.loads(clip.get_note_expression(1, 'pressure')) == values[2]
        assert json.loads(track.get_arrangement_snapshot(parameter)) == values[3]
        return {'restoredExact': True, 'transportStopped': not song.is_playing}
    finally:
        provider.close()
        state['server'].disconnect()
        if state['previous']:
            type(state['previous']).__init__(state['previous'], state['previous'].mapper, None)
        del Live._kumi_editing_e2e
        state['endpoint_path'].unlink(missing_ok=True)
