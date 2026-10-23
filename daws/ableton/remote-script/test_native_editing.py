"""Transaction guards and compensation, separate from upstream native ABI tests."""
import copy
import json
import types
import unittest
from ableton_mcp_remote_script import LiveObjectMapper, validate_operation_payload
from test_remote_script import FakeSong, FakeClip, FakeTrack, FakeScene

class NativeEditingTests(unittest.TestCase):
    def setUp(self):
        self.song = FakeSong(); self.mapper = LiveObjectMapper(self.song)
        self.mapper.willington_editing_writes = True
        self.global_enabled = False; self.calls = []
        self.song.get_follow_actions_enabled = lambda: self.global_enabled
        def global_set(value): self.calls.append(('global', value)); self.global_enabled = value
        self.song.set_follow_actions_enabled = global_set
        self.follow = dict(enabled=False, linked=True, action_a=4, action_b=0, chance_a=100., chance_b=0., jump_a=0., jump_b=0., time=4., loop_count=1.)
        self.scene = self.song.scenes[0]
        self.scene.get_follow_actions = lambda: json.dumps(self.follow)
        def scene_set(field, value):
            self.calls.append((field,value)); self.follow[field] = value
            if field.startswith('chance_'): self.follow['chance_b' if field=='chance_a' else 'chance_a'] = 100-value
        self.scene.set_follow_action = scene_set
        self.scene_ref = self.mapper.refs.put('scene', self.scene, '0')
        self.clip = FakeClip(4); self.song.tracks[0].clip_slots[0].clip = self.clip
        self.note = types.SimpleNamespace(note_id=1,pitch=60,start_time=0.,duration=1.,velocity=100.,mute=False,probability=1.,velocity_deviation=0.,release_velocity=64.)
        self.clip.get_notes_by_id = lambda ids: [self.note] if self.note and ids==(1,) else []
        self.lane = {'exists':False,'events':[], 'dimension':'pressure','unit':'midi','time_origin':'note_start'}
        self.clip.get_note_expression = lambda *a: json.dumps(self.lane)
        def expression_set(note, dimension, value):
            self.calls.append(('expression', value)); self.lane.update(json.loads(value))
        self.clip.replace_note_expression = expression_set
        self.clip_ref = self.mapper.refs.put('clip', self.clip, '0:0')
        self.track = self.song.tracks[0]; self.track.is_grouped = self.track.is_foldable = False
        self.track_ref = self.mapper.refs.put('track', self.track, '0')
        self.parameter = self.track.devices[0].parameters[0]; self.parameter.is_quantized = False
        self.parameter_ref = self.mapper.snapshot()['tracks'][0]['devices'][0]['parameters'][0]['ref']
        self.envelope = {'exists':False,'events':[], 'signature':'session-a'}
        self.track.get_arrangement_snapshot = lambda p: json.dumps(self.envelope)
        def restore(p, value):
            state = json.loads(value)
            if state['signature'] != self.envelope['signature']: raise ValueError('different adapter session')
            self.calls.append(('restore', value)); self.envelope = state
        self.track.restore_arrangement_snapshot = restore
        def insert(p, value): self.calls.append(('insert', value)); self.envelope.update(exists=True,events=[json.loads(value)])
        self.track.insert_arrangement_event = insert
        self.track.get_arrangement_automation = lambda p,s,e: json.dumps({'events':[r for r in self.envelope['events'] if s<=r[0]<=e]})
        self.track.delete_arrangement_events = lambda p,s,e: self.envelope.update(events=[r for r in self.envelope['events'] if not s<=r[0]<=e])

    def read(self, selector, edit=None):
        args = dict(selector)
        if edit is not None: args['edit']=edit
        validate_operation_payload('willington.editing.read','request',args)
        result=self.mapper.invoke('willington.editing.read',args)
        validate_operation_payload('willington.editing.read','result',result)
        return result
    def apply(self, selector, plan):
        args={**selector,'next':plan['next'],'expectedStateRevision':plan['stateRevision']}
        validate_operation_payload('willington.editing.set','request',args)
        result=self.mapper.invoke('willington.editing.set',args)
        validate_operation_payload('willington.editing.set','result',result)
        return result
    def undo(self, selector, prior, after):
        return self.apply(selector,{'next':json.dumps({'restore':prior['state']}),'stateRevision':after['stateRevision']})

    def test_global_and_coupled_scene_roundtrip_without_live_undo(self):
        for selector,edit in [({'kind':'global-follow'},{'enabled':True}),({'kind':'scene-follow','ref':self.scene_ref},{'chance_a':35,'enabled':True})]:
            with self.subTest(kind=selector['kind']):
                before=self.read(selector,edit); after=self.apply(selector,before)
                self.assertNotEqual(before['stateRevision'],after['stateRevision'])
                restored=self.undo(selector,before,after)
                self.assertEqual(before['stateRevision'],restored['stateRevision'])

    def test_expression_summary_exposes_visible_note_coordinates(self):
        selector = {'kind':'note-expression','ref':self.clip_ref,'noteId':1,'dimension':'pressure'}
        summary = json.loads(self.read(selector, {'exists':True,'events':[]})['summary'])
        self.assertEqual(summary['note'], {'pitch':60, 'start_time':0.})
        self.assertNotIn('note_id', summary['note'])

    def test_note_lane_absence_empty_curves_and_note_edit_fence(self):
        selector={'kind':'note-expression','ref':self.clip_ref,'noteId':1,'dimension':'pressure'}
        for events in ([], [[0.,42.,.2,.3,.7,.8],[1.,64.,.5,.5,.5,.5]]):
            before=self.read(selector,{'exists':True,'events':events}); after=self.apply(selector,before)
            self.assertTrue(self.lane['exists'])
            self.assertEqual(self.lane['events'],events)
            self.assertEqual(self.undo(selector,before,after)['stateRevision'],before['stateRevision'])
            self.assertFalse(self.lane['exists'])
        plan=self.read(selector,{'exists':True,'events':[]}); self.note.duration=2.
        with self.assertRaisesRegex(ValueError,'changed'): self.apply(selector,plan)

    def test_arrangement_opaque_snapshot_restore_and_session_fence(self):
        selector={'kind':'arrangement-automation','ref':self.track_ref,'targetRef':self.parameter_ref}
        before=self.read(selector,{'action':'insert','event':[4,.75,.5,.5,.5,.5]})
        after=self.apply(selector,before)
        self.assertTrue(self.envelope['exists'])
        self.assertEqual(self.undo(selector,before,after)['stateRevision'],before['stateRevision'])
        after=self.apply(selector,before); self.envelope['signature']='session-b'
        with self.assertRaisesRegex(ValueError,'changed'): self.undo(selector,before,after)
        self.assertTrue(self.envelope['exists'])

    def test_stale_identity_playback_and_disabled_provider_refuse_before_write(self):
        selector={'kind':'scene-follow','ref':self.scene_ref}; plan=self.read(selector,{'enabled':True})
        self.song.is_playing=True
        with self.assertRaisesRegex(ValueError,'stopped'): self.apply(selector,plan)
        self.song.is_playing=False; self.mapper.willington_editing_writes=False
        with self.assertRaisesRegex(ValueError,'unavailable'): self.apply(selector,plan)
        self.mapper.willington_editing_writes=True; self.song.scenes[0]=FakeScene()
        with self.assertRaisesRegex(ValueError,'identity changed'): self.apply(selector,plan)
        self.assertEqual(self.calls,[])

    def test_partial_scene_failure_restores_complete_coupled_state(self):
        selector={'kind':'scene-follow','ref':self.scene_ref}; before=self.read(selector,{'enabled':True,'chance_a':20,'loop_count':2})
        original=self.scene.set_follow_action; failed=[False]
        def fail(field,value):
            original(field,value)
            if field=='loop_count' and not failed[0]: failed[0]=True; raise RuntimeError('injected after mutation')
        self.scene.set_follow_action=fail
        with self.assertRaisesRegex(RuntimeError,'injected'): self.apply(selector,before)
        self.assertEqual(self.read(selector)['stateRevision'],before['stateRevision'])

    def test_invalid_events_and_chances_never_mutate(self):
        selector={'kind':'note-expression','ref':self.clip_ref,'noteId':1,'dimension':'pressure'}
        for events in ([[0,128,.5,.5,.5,.5]], [[0,1,.5,.5,.5,.5]]*3, [[1,1,.5,.5,.5,.5],[0,1,.5,.5,.5,.5]]):
            with self.assertRaises(ValueError): self.read(selector,{'exists':True,'events':events})
        with self.assertRaises(ValueError): self.read({'kind':'scene-follow','ref':self.scene_ref},{'chance_a':70,'chance_b':70})
        with self.assertRaisesRegex(ValueError,'linear handles'): self.read({'kind':'arrangement-automation','ref':self.track_ref,'targetRef':self.parameter_ref},{'action':'insert','event':[60,.5,.2,.3,.7,.8]})
        self.assertEqual(self.calls,[])

    def test_group_creation_returns_new_reference_and_declares_no_history_inverse(self):
        second=FakeTrack();second.is_grouped=second.is_foldable=False;self.song.tracks.append(second)
        refs=[self.track_ref,self.mapper.refs.put('track',second,'1')]
        def group_tracks(*tracks):
            group=FakeTrack();group.is_foldable=True;group.is_grouped=False
            self.song.tracks.insert(0,group)
            for track in tracks: track.is_grouped=True;track.group_track=group
            return group
        self.song.group_tracks=group_tracks
        selector={'kind':'group-tracks','trackRefs':refs}
        before=self.read(selector,{'action':'group'})
        self.assertEqual(json.loads(before['summary'])['undo'],'not undoable')
        after=self.apply(selector,before)
        self.assertEqual(json.loads(after['state'])['createdRef'],self.track_ref)
        self.assertEqual(len(self.song.tracks),3)

    def test_long_prior_expression_lane_refuses_preview_without_mutation(self):
        self.lane.update(exists=True, events=[[i / 10, 64, .5, .5, .5, .5] for i in range(5000)])
        original = copy.deepcopy(self.lane)
        selector = {'kind':'note-expression','ref':self.clip_ref,'noteId':1,'dimension':'pressure'}
        with self.assertRaisesRegex(ValueError, 'restoration limit'):
            self.read(selector, {'exists':True, 'events':[]})
        self.assertEqual(self.lane, original)
        self.assertEqual(self.calls, [])

    def test_float32_expression_time_handles_and_scene_time_roundtrip(self):
        import struct
        f32 = lambda value: struct.unpack('f', struct.pack('f', value))[0]
        original = self.clip.replace_note_expression
        def rounded(note, dimension, raw):
            value = json.loads(raw)
            value['events'] = [[f32(v) for v in row] for row in value['events']]
            original(note, dimension, json.dumps(value))
        self.clip.replace_note_expression = rounded
        selector = {'kind':'note-expression','ref':self.clip_ref,'noteId':1,'dimension':'pressure'}
        before = self.read(selector, {'exists':True,'events':[[.1, 42.1, .2, .3, .7, .8]]})
        after = self.apply(selector, before)
        self.assertEqual(self.undo(selector,before,after)['stateRevision'], before['stateRevision'])
        original_scene = self.scene.set_follow_action
        self.scene.set_follow_action = lambda field,value: original_scene(field, f32(value) if field == 'time' else value)
        selector = {'kind':'scene-follow','ref':self.scene_ref}
        before = self.read(selector, {'time':1/3})
        after = self.apply(selector,before)
        self.assertEqual(self.undo(selector,before,after)['stateRevision'], before['stateRevision'])

if __name__=='__main__': unittest.main()
