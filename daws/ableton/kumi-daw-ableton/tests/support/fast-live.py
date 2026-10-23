
import json
class Track:
    def __init__(self, name): self.name, self.canonical_parent = name, None
class Device:
    def __init__(self, name, parameters, parent):
        self.name, self.parameters, self.canonical_parent = name, parameters, parent
        for p in parameters: p.canonical_parent = self
class Parameter:
    def __init__(self, name, value, lo, hi, quantized=False, items=(), enabled=True):
        self.name, self._value, self.min, self.max, self.is_quantized, self.value_items, self.is_enabled = name, value, lo, hi, quantized, list(items), enabled
    @property
    def value(self): return self._value
    @value.setter
    def value(self, v):
        if v < self.min or v > self.max: raise ValueError('out of range')
        if self.name == 'Locked': raise RuntimeError('Live refused')
        self._value = v
    def str_for_value(self, v): return self.value_items[int(v)] if self.is_quantized else '%.1f dB' % (-36 + 48 * v)
class Refs:
    def __init__(self, objects): self.objects = objects
    def get(self, ref):
        if ref not in self.objects: raise KeyError('stale or invalid reference')
        return self.objects[ref]
class Bridge:
    def __init__(self, refs): self.refs = refs
track = Track('Bass')
device = Device('Saturator', [Parameter('Drive', 0.75, 0.0, 1.0), Parameter('Type', 0, 0, 3, True, ['Analog', 'Soft', 'Medium', 'Hard']), Parameter('Locked', 0.5, 0.0, 1.0)], track)
# A device deleted in Live: its reference still resolves, but Live refuses every use of it in its own C++ words.
class Deleted:
    def __getattr__(self, name): raise TypeError('Python argument types in None.None(Device) did not match C++ signature: None(TPyHandle<ADevice>)')
bridge = Bridge(Refs({'7:device:0:0': device, '7:parameter:0:0:1': device.parameters[1], '7:device:0:1': Deleted(), '7:parameter:0:1:0': Deleted()}))
def plain(value):
    if isinstance(value, (Track, Device, Parameter)): return {'ref': '?', 'type': type(value).__name__, 'name': value.name}
    raise TypeError(type(value).__name__)
out = []
for code in json.loads(CODES):
    env = {'bridge': bridge, 'result': None}
    try:
        exec(compile(code, '<python.run>', 'exec'), env, env)
        out.append({'ok': True, 'result': json.loads(json.dumps(env['result'], default=plain)), 'drive': device.parameters[0].value, 'type': device.parameters[1].value})
    except Exception as error:
        out.append({'ok': False, 'error': str(error), 'drive': device.parameters[0].value, 'type': device.parameters[1].value})
print(json.dumps(out))
