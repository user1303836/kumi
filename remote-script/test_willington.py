import hashlib
import json
import sys
import tempfile
import types
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch
import AbletonMcpBridge as wrapper
from test_remote_script import FakeSong, FakeClip, _protect_windows_owner_only
from ableton_mcp_remote_script import LiveObjectMapper, validate_operation_payload

class FollowClip(FakeClip):
    def __init__(self):
        super().__init__(4)
        self.is_playing = self.is_triggered = self.is_recording = False
        self.follow_action_enabled = False
        self.follow_action_linked = True
        self.follow_action_a = 4
        self.follow_action_b = 0
        self.follow_action_loop_count = 1
        self.follow_action_time = 4.0
        self.follow_action_jump_a = self.follow_action_jump_b = 1.0
        self.follow_action_chance_a = 100
    @property
    def follow_action_chance_a(self): return self._chance
    @follow_action_chance_a.setter
    def follow_action_chance_a(self, value): self._chance = float(value)
    @property
    def follow_action_chance_b(self): return 100 - self._chance

class WillingtonTests(unittest.TestCase):
    def setUp(self):
        self.song = FakeSong(); self.clip = FollowClip()
        self.song.tracks[0].clip_slots[0].clip = self.clip
        self.mapper = LiveObjectMapper(self.song)
        self.mapper.willington_follow_writes = True
        self.row = self.mapper.snapshot()['tracks'][0]['clips'][0]
    def args(self, **changes):
        before = self.mapper._follow_action_fields(self.clip)
        return {**before, **changes, 'ref': self.row['ref'], 'expectedObjectIdentity': self.row['objectIdentity'],
                'expectedAuthorityRevision': self.mapper._clip_authority_digest(self.row['ref']),
                'expectedStateRevision': hashlib.sha256(self.mapper._bounded_canonical(before).encode()).hexdigest()}
    def test_follow_capability_is_available_before_the_first_clip_exists(self):
        self.song.tracks[0].clip_slots[0].clip = None
        self.assertTrue(self.mapper._operation_supported('clip.follow-actions.set'))
        self.song.tracks[0].clip_slots[0].clip = FakeClip(4)
        self.row = self.mapper.snapshot()['tracks'][0]['clips'][0]
        self.assertTrue(self.mapper._operation_supported('clip.follow-actions.set'))
        with self.assertRaisesRegex(ValueError, 'unavailable'):
            self.mapper._follow_action_set(self.args())
        self.mapper.willington_follow_writes = False
        self.assertFalse(self.mapper._operation_supported('clip.follow-actions.set'))

    def test_follow_read_write_and_explicit_restoration(self):
        self.assertTrue(self.mapper._operation_supported('clip.follow-actions.set'))
        prior = self.mapper._follow_action_fields(self.clip)
        args = self.args(followActionChanceA=75, followActionChanceB=25, followActionEnabled=True)
        validate_operation_payload('clip.follow-actions.set', 'request', args)
        result = self.mapper.invoke('clip.follow-actions.set', args)
        validate_operation_payload('clip.follow-actions.set', 'result', result)
        self.assertEqual(self.clip.follow_action_chance_b, 25)
        self.mapper.invoke('clip.follow-actions.set', self.args(**prior))
        self.assertEqual(self.mapper._follow_action_fields(self.clip), prior)
    def test_refuse_disabled_stale_playing_and_invalid(self):
        self.mapper.willington_follow_writes = False
        self.assertFalse(self.mapper._operation_supported('clip.follow-actions.set'))
        with self.assertRaises(ValueError): self.mapper._follow_action_set(self.args())
        self.mapper.willington_follow_writes = True
        stale = self.args(); self.clip.follow_action_a = 5
        with self.assertRaisesRegex(ValueError, 'state changed'): self.mapper._follow_action_set(stale)
        for values in ({'followActionChanceA': 75}, {'followActionA': True}, {'followActionJumpA': 0}, {'followActionLoopCount': 1.5}):
            with self.assertRaises(ValueError): self.mapper._follow_action_set(self.args(**values))
        self.song.is_playing = True
        with self.assertRaisesRegex(ValueError, 'stopped'): self.mapper._follow_action_set(self.args())
    def test_stopped_transport_allows_retained_session_flags_but_not_recording(self):
        for attribute in ('is_playing', 'is_triggered'):
            setattr(self.clip, attribute, True)
            prior = self.mapper._follow_action_fields(self.clip)
            self.mapper._follow_action_set(self.args(followActionA=8))
            self.mapper._follow_action_set(self.args(**prior))
            self.assertTrue(getattr(self.clip, attribute))
            setattr(self.clip, attribute, False)
        self.clip.is_recording = True
        with self.assertRaisesRegex(ValueError, 'non-recording clip'):
            self.mapper._follow_action_set(self.args(followActionA=8))
        self.clip.is_recording = False
        prepared = self.args(followActionA=8)
        self.song.is_playing = True
        with self.assertRaisesRegex(ValueError, 'stopped transport'):
            self.mapper._follow_action_set(prepared)
        self.assertEqual(self.clip.follow_action_a, 4)

    def test_partial_write_restores_all_fields(self):
        prior = self.mapper._follow_action_fields(self.clip)
        original = self.clip.__class__
        class FailingClip(original):
            fail = True
            def __setattr__(self, key, value):
                if key == 'follow_action_loop_count' and value == 2 and self.fail:
                    self.fail = False
                    raise RuntimeError('injected')
                super().__setattr__(key, value)
        self.clip.__class__ = FailingClip
        with self.assertRaisesRegex(RuntimeError, 'injected'):
            self.mapper._follow_action_set(self.args(followActionA=7, followActionLoopCount=2))
        self.assertEqual(self.mapper._follow_action_fields(self.clip), prior)

class ZoneTests(unittest.TestCase):
    def setUp(self):
        DeviceTests.setUp(self)
        import json
        self.rack.class_name = 'InstrumentGroupDevice'
        self.chain = self.rack.chains[0]
        self.zone = {'minimum':12, 'maximum':104, 'fadeMinimum':24, 'fadeMaximum':88,
                     'lowerBound':0, 'upperBound':127}
        self.chain.get_zone = lambda kind: json.dumps(self.zone)
        def write(kind, minimum, maximum, fade_minimum, fade_maximum):
            self.zone.update(minimum=minimum, maximum=maximum,
                             fadeMinimum=fade_minimum, fadeMaximum=fade_maximum)
        self.chain.set_zone = write
        self.mapper.willington_zone_writes = True
        self.row = self.mapper.snapshot()['tracks'][0]['devices'][0]
        self.selector = {'ref':self.row['ref'], 'kind':'selector-zone',
                         'targetRef':self.row['chains'][0]['ref']}

    def test_audio_racks_refuse_hidden_key_and_velocity_zones(self):
        self.rack.class_name = 'AudioEffectGroupDevice'
        for kind in ['key-zone', 'velocity-zone']:
            with self.assertRaisesRegex(ValueError, 'selector zones only'):
                self.mapper._willington_device_read({**self.selector, 'kind': kind})
        self.mapper._willington_device_read(self.selector)

    def test_zone_complete_state_and_restore(self):
        before = self.mapper.invoke('willington.device.read', self.selector)
        next_state = {'minimum':16, 'maximum':40, 'fadeMinimum':20, 'fadeMaximum':36}
        args = {**self.selector, 'next':next_state, 'expectedStateRevision':before['stateRevision']}
        validate_operation_payload('willington.device.set', 'request', args)
        after = self.mapper.invoke('willington.device.set', args)
        validate_operation_payload('willington.device.set', 'result', after)
        self.assertEqual({key:after['state'][key] for key in next_state}, next_state)
        restored = self.mapper.invoke('willington.device.set', {**self.selector,
            'next':{key:before['state'][key] for key in next_state},
            'expectedStateRevision':after['stateRevision']})
        self.assertEqual(restored['stateRevision'], before['stateRevision'])

    def test_zone_refuses_stale_detached_disabled_and_bad_endpoints(self):
        before = self.mapper.invoke('willington.device.read', self.selector)
        args = {**self.selector, 'next':{'minimum':16,'maximum':40,'fadeMinimum':20,'fadeMaximum':36},
                'expectedStateRevision':before['stateRevision']}
        self.zone['fadeMaximum'] = 86
        with self.assertRaisesRegex(ValueError, 'changed'):
            self.mapper.invoke('willington.device.set', args)
        self.mapper.willington_zone_writes = False
        with self.assertRaisesRegex(ValueError, 'unavailable'):
            self.mapper.invoke('willington.device.read', self.selector)
        self.mapper.willington_zone_writes = True
        current = self.mapper.invoke('willington.device.read', self.selector)
        for value in (True, 20.5, -1, 50):
            bad = {**args, 'expectedStateRevision':current['stateRevision'],
                   'next':{**args['next'],'fadeMinimum':value}}
            with self.assertRaises(ValueError): self.mapper.invoke('willington.device.set', bad)
        self.rack.chains = []
        with self.assertRaisesRegex(ValueError, 'no longer'):
            self.mapper.invoke('willington.device.read', self.selector)

    def test_zone_partial_failure_restores_four_endpoints(self):
        before = dict(self.zone); write = self.chain.set_zone; fail = [True]
        def partial(kind, *values):
            if fail[0]:
                fail[0] = False
                self.zone['minimum'] = values[0]
                raise RuntimeError('injected zone failure')
            write(kind, *values)
        self.chain.set_zone = partial
        current = self.mapper.invoke('willington.device.read', self.selector)
        with self.assertRaisesRegex(RuntimeError, 'injected'):
            self.mapper.invoke('willington.device.set', {**self.selector,
                'next':{'minimum':16,'maximum':40,'fadeMinimum':20,'fadeMaximum':36},
                'expectedStateRevision':current['stateRevision']})
        self.assertEqual(self.zone, before)

class DeviceTests(unittest.TestCase):
    def setUp(self):
        import json
        from test_remote_script import FakeRackDevice, FakeDevice
        self.song = FakeSong(); self.rack = FakeRackDevice(); self.target = FakeDevice()
        self.rack.chains = [type('Chain', (), {'name':'Chain', 'devices':[self.target], 'mute':False, 'solo':False})()]
        self.song.tracks[0].devices = [self.rack]
        self.mapper = LiveObjectMapper(self.song); self.mapper.willington_device_writes = True
        self.mapping = None; self.name = 'Variation 1'
        self.rack.rename_macro = lambda index, name: setattr(self.rack.macros[index], 'name', name)
        self.rack.get_selected_variation_name = lambda: self.name
        self.rack.rename_selected_variation = lambda name: setattr(self, 'name', name)
        self.rack.get_macro_mapping = lambda target: json.dumps(self.mapping)
        self.rack.map_macro = lambda index, target: setattr(self, 'mapping', {'index':index,'minimum':0.,'maximum':1.,'kind':'continuous'})
        self.rack.set_macro_mapping_range = lambda target, low, high: self.mapping.update(minimum=low,maximum=high)
        self.rack.unmap_macro = lambda target: setattr(self, 'mapping', None)
        self.row = self.mapper.snapshot()['tracks'][0]['devices'][0]
        self.ref = self.row['ref']; self.target_ref = self.row['chains'][0]['devices'][0]['parameters'][0]['ref']
    def selector(self, kind):
        return {'ref':self.ref,'kind':kind, **({'macroIndex':0} if kind=='macro-name' else {'targetRef':self.target_ref} if kind=='macro-mapping' else {})}
    def apply(self, kind, next):
        selector = self.selector(kind)
        before = self.mapper.invoke('willington.device.read', selector)
        args = {**selector,'next':next,'expectedStateRevision':before['stateRevision']}
        validate_operation_payload('willington.device.set','request',args)
        result = self.mapper.invoke('willington.device.set',args)
        validate_operation_payload('willington.device.set','result',result)
        return before,result
    def test_chain_mixer_discovery_is_bounded_and_rejects_detached_parent(self):
        from test_remote_script import FakeMixerDevice
        mixer = FakeMixerDevice(); mixer.panning.name = 'Chain Pan'; mixer.panning.min = -1
        self.rack.chains[0].mixer_device = mixer
        chain = self.mapper.snapshot()['tracks'][0]['devices'][0]['chains'][0]
        page = self.mapper.discover('parameter', parent=chain['ref'], limit=2)
        self.assertTrue(page['truncated'])
        rows = self.mapper.discover('parameter', parent=chain['ref'])['items']
        pan = next(row for row in rows if row['name']=='Chain Pan')
        self.assertEqual(pan['ref'],chain['mixer']['panningRef'])
        self.assertEqual((pan['min'],pan['max'],pan['parentRef']),(-1,1,chain['ref']))
        self.rack.chains = []
        with self.assertRaisesRegex(ValueError,'no longer authoritative'):
            self.mapper.discover('parameter', parent=chain['ref'])

    def test_names_apply_and_restore(self):
        for kind in ['macro-name','variation-name']:
            before,after = self.apply(kind, {'name':'Kumi 測試'})
            self.assertEqual(after['state']['name'],'Kumi 測試')
            _, restored = self.apply(kind, {'name':before['state']['name']})
            self.assertEqual(restored['stateRevision'],before['stateRevision'])
    def test_mapping_apply_and_restore(self):
        before,after = self.apply('macro-mapping', {'mapping':{'index':0,'minimum':0.75,'maximum':0.25,'kind':'continuous'},'parameterValue':0.5})
        self.assertEqual(self.mapping['minimum'],0.75)
        _,restored = self.apply('macro-mapping', {'mapping':None,'parameterValue':before['state']['parameterValue']})
        self.assertEqual(restored['stateRevision'], before['stateRevision'])
    def test_mapping_failure_compensates(self):
        self.rack.set_macro_mapping_range = lambda *args: (_ for _ in ()).throw(RuntimeError('injected'))
        with self.assertRaisesRegex(RuntimeError,'injected'):
            self.apply('macro-mapping', {'mapping':{'index':0,'minimum':0.75,'maximum':0.25,'kind':'continuous'},'parameterValue':0.5})
        self.assertIsNone(self.mapping)
    def test_stale_detached_invalid_and_playing(self):
        selector = self.selector('macro-name'); before = self.mapper.invoke('willington.device.read',selector)
        self.rack.macros[0].name = 'External edit'
        with self.assertRaisesRegex(ValueError,'changed'):
            self.mapper.invoke('willington.device.set',{**selector,'next':{'name':'New'},'expectedStateRevision':before['stateRevision']})
        with self.assertRaisesRegex(ValueError,'selector'):
            self.mapper.invoke('willington.device.read',{**selector,'targetRef':self.target_ref})
        self.song.is_playing = True
        with self.assertRaisesRegex(ValueError,'stopped'): self.apply('macro-name', {'name':'New'})
        self.song.is_playing = False; self.rack.chains[0].devices = []
        with self.assertRaisesRegex(ValueError,'no longer'): self.apply('macro-mapping',{'mapping':None,'parameterValue':0.5})

    def test_variation_rename_requires_restorable_selected_name(self):
        for selected, count, name in [(-1, 1, 'Name'), (0, 0, 'Name'), (0, 1, None), (0, 1, '')]:
            self.rack.selected_variation_index = selected
            self.rack.variation_count = count
            self.name = name
            with self.assertRaisesRegex(ValueError, 'variation must be selected'):
                self.mapper._willington_device_read({'ref': self.ref, 'kind': 'variation-name'})

class ComponentUnavailableError(RuntimeError): pass


class _ProviderFixture:
    """Native doubles preserve process registration and remove only owned patches."""
    components = ('WillingtonBindings', 'WillingtonDeviceTools', 'WillingtonRackZones')
    labels = ('follow', 'devices', 'zones')

    def __init__(self, root):
        self.root = root
        self.path = root / 'willington.json'
        self.mapper = types.SimpleNamespace()
        self.live = types.ModuleType('Live')
        self.chain_class = type('Chain', (), {})
        self.device_class = type('Device', (), {})
        self.live.Chain = types.SimpleNamespace(Chain=self.chain_class)
        self.calls = []; self.logs = []; self.providers = []
        self.library = root / 'build' / 'selected-profile' / 'libwillington.dylib'
        self.library.parent.mkdir(parents=True)
        self.library.write_bytes(b'selected native library')
        self.root_library = root / 'libwillington.dylib'
        self.root_library.write_bytes(b'legacy native library')
        self.follow = types.SimpleNamespace(path=str(self.library),
            willington_enable_writes=lambda value: self.calls.append(('follow', value)))
        self.devices = self.patchable('devices', self.device_class, ('willington_test_method',))
        self.zones = self.patchable('zones', self.chain_class, ('get_zone', 'set_zone'))
        bindings = types.ModuleType('WillingtonBindings')
        bindings.__file__ = str(root / 'bindings.py'); bindings.install = self.install_follow
        devices_api = types.ModuleType('WillingtonDeviceTools.api'); devices_api.install = self.install_devices
        zones_api = types.ModuleType('WillingtonRackZones.api'); zones_api.install = self.install_zones
        runtime = types.ModuleType('WillingtonRuntime'); runtime.ComponentUnavailableError = ComponentUnavailableError
        self.modules = {'Live': self.live, 'WillingtonBindings': bindings,
            'WillingtonDeviceTools': types.ModuleType('WillingtonDeviceTools'),
            'WillingtonDeviceTools.api': devices_api,
            'WillingtonRackZones': types.ModuleType('WillingtonRackZones'),
            'WillingtonRackZones.api': zones_api, 'WillingtonRuntime': runtime}
        self.write_receipt()

    def write_config(self, config):
        self.path.write_text(json.dumps(config))
        self.path.chmod(0o600)
        _protect_windows_owner_only(self.path)

    def write_receipt(self, payload=None):
        payload = self.library.read_bytes() if payload is None else payload
        (self.root / 'self-test.json').write_text(json.dumps({
            'status': 'passed', 'library_sha256': hashlib.sha256(payload).hexdigest()}))

    def patchable(self, label, cls, names):
        native = types.SimpleNamespace(patches=[],
            enable=lambda value: self.calls.append((label, value)))
        def uninstall():
            self.calls.append((label, 'uninstall'))
            for owner, name, method in reversed(native.patches):
                if getattr(owner, name, None) is method: delattr(owner, name)
            native.patches = []
        native.uninstall = uninstall
        native.fixture_class = cls; native.fixture_names = names
        return native

    def install_follow(self):
        self.calls.append(('follow', 'install'))
        self.live._willington_native_library = self.follow
        return self.follow

    def install_patches(self, label, native):
        self.calls.append((label, 'install'))
        for name in native.fixture_names:
            method = lambda *args: None
            setattr(native.fixture_class, name, method)
            native.patches.append((native.fixture_class, name, method))
        attribute = '_willington_' + ('device' if label == 'devices' else 'zone') + '_libraries'
        setattr(self.live, attribute, [native])
        return native

    def install_devices(self): return self.install_patches('devices', self.devices)
    def install_zones(self): return self.install_patches('zones', self.zones)

    def fail_install(self, component, error):
        position = self.components.index(component)
        module = component if position == 0 else component + '.api'
        def fail():
            self.calls.append((self.labels[position], 'install'))
            raise error
        self.modules[module].install = fail

    def construct(self):
        provider = wrapper._WillingtonProvider(self.mapper, self.logs.append)
        self.providers.append(provider)
        return provider


@contextmanager
def _provider_fixture(config=None, modules=None):
    with tempfile.TemporaryDirectory() as folder:
        fixture = _ProviderFixture(Path(folder))
        if config is not None:
            fixture.write_config({'version': 1, 'followActions': False, 'deviceTools': False,
                                  'enableWrites': False, **config})
        fixture.modules.update(modules or {})
        with patch.object(wrapper, '__file__', str(fixture.root / '__init__.py')), \
                patch.dict('sys.modules', fixture.modules):
            try: yield fixture
            finally:
                for provider in fixture.providers: provider.close()


class ProviderTests(unittest.TestCase):
    def assert_closed(self, fixture, provider):
        for name in ('willington_follow_writes', 'willington_device_writes', 'willington_zone_writes'):
            self.assertFalse(getattr(fixture.mapper, name))
        for name in ('follow', 'devices', 'zones'): self.assertIsNone(getattr(provider, name))
        self.assertIsNone(getattr(fixture.live, '_kumi_willington_owner', None))

    def test_absent_invalid_and_duplicate_owner_fail_closed(self):
        with _provider_fixture() as fixture:
            owner = fixture.live._kumi_willington_owner = object()
            fixture.construct()
            self.assertFalse(fixture.mapper.willington_follow_writes)
            fixture.write_config({})
            fixture.construct()
            self.assertFalse(fixture.mapper.willington_device_writes)
            fixture.write_config({'version': 1, 'followActions': False, 'deviceTools': False, 'enableWrites': False})
            fixture.construct()
            self.assertIs(fixture.live._kumi_willington_owner, owner)
            self.assertEqual(len(fixture.logs), 2)

    def test_missing_follow_self_test_preserves_independent_device_writes(self):
        with _provider_fixture({'followActions': True, 'deviceTools': True, 'enableWrites': True}) as fixture:
            (fixture.root / 'self-test.json').unlink()
            provider = fixture.construct()
            self.assertFalse(fixture.mapper.willington_follow_writes)
            self.assertTrue(fixture.mapper.willington_device_writes)
            self.assertNotIn(('follow', True), fixture.calls)
            self.assertIn(('devices', True), fixture.calls)
            self.assertTrue(any('self-test.json' in line for line in fixture.logs))
            self.assertEqual(fixture.logs[-1], 'Willington extensions initialized; active providers: Follow Actions, Device Tools; writes enabled: Device Tools')
            provider.close()
            self.assert_closed(fixture, provider)

    def test_each_typed_component_refusal_preserves_others_and_is_cached(self):
        for refused in _ProviderFixture.components:
            for writes in (False, True):
                with self.subTest(component=refused, writes=writes), _provider_fixture({
                        'followActions': True, 'deviceTools': True, 'rackZones': True, 'enableWrites': writes}) as fixture:
                    refusal = 'Willington %s unavailable for macos/arm64 Live 12.4.15b4: 0 exact validated profiles' % refused
                    fixture.fail_install(refused, ComponentUnavailableError(refusal))
                    for attempt in range(2):
                        provider = fixture.construct()
                        self.assertIs(fixture.live._kumi_willington_owner, provider)
                        for component, label, flag in zip(fixture.components, fixture.labels,
                                ('willington_follow_writes', 'willington_device_writes', 'willington_zone_writes')):
                            active = component != refused
                            self.assertEqual(getattr(provider, label) is not None, active)
                            self.assertEqual(getattr(fixture.mapper, flag), active and writes)
                            self.assertEqual(fixture.calls.count((label, True)), (attempt + 1) * int(active and writes))
                        self.assertEqual(fixture.live._kumi_willington_unavailable_components, {refused})
                        self.assertEqual(fixture.logs.count(refusal), 1)
                        self.assertFalse(any('extensions unavailable:' in line for line in fixture.logs))
                        if refused == 'WillingtonBindings':
                            self.assertIsNone(getattr(fixture.live, '_willington_native_library', None))
                        if refused == 'WillingtonDeviceTools': self.assertFalse(fixture.devices.patches)
                        if refused == 'WillingtonRackZones':
                            self.assertFalse(callable(getattr(fixture.chain_class, 'get_zone', None)))
                            self.assertFalse(callable(getattr(fixture.chain_class, 'set_zone', None)))
                        provider.close(); provider.close()
                        self.assert_closed(fixture, provider)
                    for component, label in zip(fixture.components, fixture.labels):
                        expected = 1 if component == refused or label == 'follow' else 2
                        self.assertEqual(fixture.calls.count((label, 'install')), expected)
                    for label, component in zip(fixture.labels[1:], fixture.components[1:]):
                        self.assertEqual(fixture.calls.count((label, 'uninstall')), 0 if component == refused else 2)

    def test_all_typed_refusals_log_zero_active_and_zero_writable_providers(self):
        with _provider_fixture({'followActions': True, 'deviceTools': True,
                                'rackZones': True, 'enableWrites': True}) as fixture:
            for component in fixture.components:
                fixture.fail_install(component, ComponentUnavailableError(component + ': no exact validated profile'))
            for _ in range(2):
                provider = fixture.construct()
                self.assertEqual(fixture.logs[-1], 'Willington extensions initialized; active providers: none; writes enabled: none')
                self.assertIs(fixture.live._kumi_willington_owner, provider)
                provider.close(); self.assert_closed(fixture, provider)
            self.assertEqual(fixture.live._kumi_willington_unavailable_components, set(fixture.components))
            for component, label in zip(fixture.components, fixture.labels):
                self.assertEqual(fixture.calls.count((label, 'install')), 1)
                self.assertEqual(fixture.logs.count(component + ': no exact validated profile'), 1)
                self.assertNotIn((label, True), fixture.calls)

    def test_hard_install_failures_retry_and_tear_down_with_any_runtime(self):
        runtime_cases = ('typed', 'absent', 'missing-symbol')
        for runtime in runtime_cases:
            modules = {} if runtime == 'typed' else {'WillingtonRuntime':
                None if runtime == 'absent' else types.ModuleType('WillingtonRuntime')}
            for failed in _ProviderFixture.components:
                with self.subTest(runtime=runtime, component=failed), _provider_fixture({
                        'followActions': True, 'deviceTools': True, 'rackZones': True, 'enableWrites': True}, modules) as fixture:
                    fixture.fail_install(failed, RuntimeError('Unsupported executable or library hash'))
                    for attempt in range(2):
                        provider = fixture.construct()
                        self.assert_closed(fixture, provider)
                        self.assertFalse(getattr(fixture.live, '_kumi_willington_unavailable_components', ()))
                        self.assertTrue(any('extensions unavailable: Unsupported executable or library hash' in line for line in fixture.logs))
                        failed_label = fixture.labels[fixture.components.index(failed)]
                        self.assertEqual(fixture.calls.count((failed_label, 'install')), attempt + 1)
                        self.assertEqual(fixture.calls.count(('devices', 'uninstall')),
                                         attempt + 1 if failed == 'WillingtonRackZones' else 0)
                        self.assertFalse(fixture.devices.patches); self.assertFalse(fixture.zones.patches)
                        provider.close()

    def test_enable_failure_disables_and_uninstalls_acquired_providers_without_caching(self):
        for failed_label in ('devices', 'zones'):
            with self.subTest(component=failed_label), _provider_fixture({
                    'followActions': True, 'deviceTools': True, 'rackZones': True, 'enableWrites': True}) as fixture:
                def fail(value): raise RuntimeError('unexpected enable failure')
                getattr(fixture, failed_label).enable = fail
                for _ in range(2):
                    provider = fixture.construct()
                    self.assert_closed(fixture, provider)
                    self.assertFalse(getattr(fixture.live, '_kumi_willington_unavailable_components', ()))
                    self.assertIn(('follow', True), fixture.calls)
                    self.assertEqual(fixture.calls[-3:], [('follow', False), ('devices', 'uninstall'), ('zones', 'uninstall')])
                    self.assertFalse(fixture.devices.patches); self.assertFalse(fixture.zones.patches)
                self.assertEqual(fixture.calls.count((failed_label, 'install')), 2)

    def test_willingtons_identity_is_hashed_once_while_live_runs(self):
        # identity() SHA-256s the whole Live executable; the components' installs ask for it several times a provider.
        runtime = types.ModuleType('WillingtonRuntime'); hashed = []
        runtime.identity = lambda: hashed.append(True) or {'platform': 'macos', 'source_sha256': 'f' * 64}
        with _provider_fixture({'followActions': True, 'deviceTools': True, 'enableWrites': False}, {'WillingtonRuntime': runtime}) as fixture:
            fixture.construct(); fixture.construct()
            first = runtime.identity(); first['platform'] = 'changed'
            self.assertEqual(runtime.identity()['platform'], 'macos', "each caller gets its own copy")
            self.assertEqual(len(hashed), 1, "hashed once, however many providers and installs ask")
        wrapper._keep_willington_identity(types.ModuleType('WillingtonRuntime'))  # a runtime without identity is left alone

    def test_legacy_packages_without_typed_runtime_still_install(self):
        for runtime in (None, types.ModuleType('WillingtonRuntime')):
            with self.subTest(runtime=runtime), _provider_fixture({'followActions': True,
                    'deviceTools': True, 'rackZones': True, 'enableWrites': True}, {'WillingtonRuntime': runtime}) as fixture:
                provider = fixture.construct()
                self.assertIs(fixture.live._kumi_willington_owner, provider)
                for label in fixture.labels:
                    self.assertIs(getattr(provider, label), getattr(fixture, label))
                    self.assertIn((label, 'install'), fixture.calls)
                    self.assertIn((label, True), fixture.calls)
                self.assertFalse(any('unavailable' in line for line in fixture.logs))
                provider.close(); provider.close(); self.assert_closed(fixture, provider)
                self.assertEqual(fixture.calls.count(('devices', 'uninstall')), 1)
                self.assertEqual(fixture.calls.count(('zones', 'uninstall')), 1)

    def test_follow_evidence_uses_selected_profile_library(self):
        for matching in (False, True):
            with self.subTest(matching=matching), _provider_fixture({'followActions': True, 'enableWrites': True}) as fixture:
                fixture.write_receipt(fixture.library.read_bytes() if matching else fixture.root_library.read_bytes())
                provider = fixture.construct()
                self.assertEqual(fixture.mapper.willington_follow_writes, matching)
                self.assertEqual(fixture.calls.count(('follow', True)), int(matching))
                self.assertEqual(fixture.logs[-1], 'Willington extensions initialized; active providers: Follow Actions; writes enabled: ' + ('Follow Actions' if matching else 'none'))
                provider.close()

    def test_pathless_legacy_follow_uses_matching_root_library_receipt(self):
        with _provider_fixture({'followActions': True, 'enableWrites': True}) as fixture:
            del fixture.follow.path
            fixture.write_receipt(fixture.root_library.read_bytes())
            provider = fixture.construct()
            self.assertTrue(fixture.mapper.willington_follow_writes)
            self.assertIn(('follow', True), fixture.calls)
            self.assertIs(provider.follow, fixture.follow)
            provider.close(); self.assert_closed(fixture, provider)

    def test_device_owner_enable_and_teardown(self):
        with _provider_fixture({'deviceTools': True, 'enableWrites': True}) as fixture:
            provider = fixture.construct()
            self.assertTrue(fixture.mapper.willington_device_writes)
            self.assertIs(fixture.live._kumi_willington_owner, provider)
            provider.close(); provider.close()
            self.assertEqual(fixture.calls, [('devices', 'install'), ('devices', True), ('devices', 'uninstall')])
            self.assert_closed(fixture, provider)

    def test_follow_reconnect_reuses_realistic_disabled_native_registration(self):
        with _provider_fixture({'followActions': True}) as fixture:
            first = fixture.construct()
            self.assertIs(fixture.live._willington_native_library, fixture.follow)
            self.assertTrue(fixture.live._kumi_willington_registered)
            first.close(); first.close()
            second = fixture.construct()
            self.assertIs(second.follow, fixture.follow)
            self.assertIs(fixture.live._kumi_willington_follow_library, fixture.follow)
            self.assertTrue(fixture.live._kumi_willington_registered)
            self.assertEqual(fixture.calls, [('follow', 'install'), ('follow', False), ('follow', False), ('follow', False)])
            self.assertFalse(any('unavailable' in line for line in fixture.logs))
            second.close(); self.assert_closed(fixture, second)

    def test_standalone_native_registrations_are_not_replaced_or_uninstalled(self):
        for label in _ProviderFixture.labels:
            with self.subTest(component=label), _provider_fixture({}) as fixture:
                getattr(fixture, 'install_' + label)()
                provider = fixture.construct()
                self.assert_closed(fixture, provider)
                self.assertEqual(fixture.calls, [(label, 'install')])
                if label == 'follow': self.assertIs(fixture.live._willington_native_library, fixture.follow)
                else: self.assertTrue(getattr(fixture, label).patches)
                self.assertTrue(any('already installed' in line for line in fixture.logs))


class SwitchTests(unittest.TestCase):
    """Kumi's /willington writes or removes willington.json while Live runs."""

    def surface(self, fixture):
        surface = object.__new__(wrapper.AbletonMcpBridge)
        surface._bridge = types.SimpleNamespace(mapper=fixture.mapper)
        surface._willington = None
        surface.log_message = fixture.logs.append
        return surface

    def test_kumis_bundled_copy_is_found_after_any_installed_beside_the_package(self):
        with _provider_fixture({'deviceTools': True, 'enableWrites': True}) as fixture, patch.object(sys, 'path', list(sys.path)):
            bundled = str(fixture.root / 'willington')
            fixture.construct().close()
            self.assertNotIn(bundled, sys.path)
            (fixture.root / 'willington').mkdir()
            for _ in range(2):
                fixture.construct().close()
            self.assertEqual(sys.path[-1], bundled)
            self.assertEqual(sys.path.count(bundled), 1)

    def test_a_changed_switch_reloads_the_provider_without_restarting_live(self):
        with _provider_fixture() as fixture:
            surface = self.surface(fixture)
            clock = [100.0]
            with patch.object(wrapper.time, 'monotonic', lambda: clock[0]):
                surface._keep_willington()
                self.assertFalse(fixture.mapper.willington_device_writes)
                fixture.write_config({'version': 1, 'followActions': False, 'deviceTools': True, 'enableWrites': True})
                surface._keep_willington()
                self.assertFalse(fixture.mapper.willington_device_writes, 'it looks once a second, not every tick')
                clock[0] += 1.5
                surface._keep_willington()
                self.assertTrue(fixture.mapper.willington_device_writes)
                self.assertEqual(fixture.calls, [('devices', 'install'), ('devices', True)])
                clock[0] += 1.5
                surface._keep_willington()
                self.assertEqual(fixture.calls.count(('devices', 'install')), 1, 'an unchanged switch keeps its provider')
                fixture.path.unlink()
                clock[0] += 1.5
                surface._keep_willington()
                self.assertFalse(fixture.mapper.willington_device_writes)
                self.assertEqual(fixture.calls[-1], ('devices', 'uninstall'))
                self.assertFalse(fixture.devices.patches)
                self.assertIn('Willington extensions off: willington.json was removed', fixture.logs)
                self.assertIsNone(getattr(fixture.live, '_kumi_willington_owner', None))
            surface._willington.close()


class ReviewRegressions(unittest.TestCase):
    def test_follow_time_uses_float32_readback(self):
        from test_remote_script import float32
        class RoundedClip(FollowClip):
            def __setattr__(self, name, value):
                super().__setattr__(name, float32(value) if name == 'follow_action_time' else value)
        test = WillingtonTests(); test.setUp()
        test.clip.__class__ = RoundedClip
        test.mapper._follow_action_set(test.args(followActionTime=1.333))
        self.assertEqual(test.clip.follow_action_time, float32(1.333))

    def test_follow_reads_only_for_enabled_session_clips(self):
        from unittest.mock import patch
        song = FakeSong(); clip = FollowClip()
        song.tracks[0].clip_slots[0].clip = clip
        song.tracks[0].arrangement_clips = [clip]
        mapper = LiveObjectMapper(song)
        with patch.object(mapper, '_follow_action_fields', side_effect=AssertionError('unexpected Follow reads')):
            snapshot = mapper.snapshot()
            self.assertNotIn('followActionA', snapshot['tracks'][0]['clips'][0])
        mapper.willington_follow_writes = True
        with patch.object(mapper, '_follow_action_fields', wraps=mapper._follow_action_fields) as read:
            snapshot = mapper.snapshot()
            self.assertEqual(read.call_count, 0)
            self.assertNotIn('followActionA', snapshot['tracks'][0]['clips'][0])
            row = snapshot['tracks'][0]['clips'][0]
            page = mapper.discover('session_clip', parent=row['ref'].replace(':clip:', ':clip_slot:'), requested_fields=['ref', 'followActionA'])
            self.assertEqual(page['items'][0]['followActionA'], 4)
            self.assertEqual(read.call_count, 1)
            self.assertNotIn('followActionA', snapshot['arrangement']['clips'][0])

    def test_macro_refs_are_canonical_and_readable(self):
        from test_remote_script import FakeRackDevice, FakeParameter
        song = FakeSong(); rack = FakeRackDevice()
        macros = [FakeParameter() for _ in range(16)]
        rack.parameters = [FakeParameter()] + macros
        del rack.macros
        rack.macros_mapped = [False] * 16
        song.tracks[0].devices = [rack]
        mapper = LiveObjectMapper(song)
        row = mapper.snapshot()['tracks'][0]['devices'][0]
        self.assertEqual([m['ref'] for m in row['macros']], [p['ref'] for p in row['parameters'][1:]])
        for macro in row['macros']:
            self.assertEqual(mapper.get(macro['ref'])['objectIdentity'], macro['objectIdentity'])
        authority = mapper._realtime_parameter_authority(row['macros'][0]['ref'])
        self.assertEqual(len({p['objectIdentity'] for p in authority['siblings']}), len(authority['siblings']))

    def test_mapping_rounding_remap_and_rollback_without_full_set_reads(self):
        from test_remote_script import float32
        from unittest.mock import patch
        import json
        test = DeviceTests(); test.setUp()
        parameter = test.target.parameters[0]; parameter.max = 20000
        test.rack.set_macro_mapping_range = lambda target, low, high: test.mapping.update(minimum=float32(low), maximum=float32(high))
        original_map = test.rack.map_macro
        def map_once(index, target):
            if test.mapping is not None: raise AssertionError('must unmap before remapping')
            original_map(index, target)
        test.rack.map_macro = map_once
        with patch.object(test.mapper, 'snapshot', side_effect=AssertionError('full snapshot forbidden')):
            test.apply('macro-mapping', {'mapping': {'index': 0, 'minimum': 2500.7, 'maximum': 12000.3, 'kind': 'continuous'}, 'parameterValue': 0.5})
            old = dict(test.mapping)
            test.apply('macro-mapping', {'mapping': {'index': 0, 'minimum': 2000.2, 'maximum': 10000.1, 'kind': 'continuous'}, 'parameterValue': 0.5})
            prior = dict(test.mapping)
            original_range = test.rack.set_macro_mapping_range
            def fail_new(target, low, high):
                if low == 3000.4: raise RuntimeError('injected range failure')
                original_range(target, low, high)
            test.rack.set_macro_mapping_range = fail_new
            with self.assertRaisesRegex(RuntimeError, 'injected range failure'):
                test.apply('macro-mapping', {'mapping': {'index': 0, 'minimum': 3000.4, 'maximum': 9000.1, 'kind': 'continuous'}, 'parameterValue': 0.5})
            self.assertEqual(test.mapping, prior)
            test.apply('macro-mapping', {'mapping': None, 'parameterValue': 2500.7})
            self.assertIsNone(test.mapping)
