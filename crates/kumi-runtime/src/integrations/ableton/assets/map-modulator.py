def why(error):
    if isinstance(error, KeyError): return "Live's references changed since Kumi read them; discover again"
    if isinstance(error, (TypeError, RuntimeError)): return "that device isn't in Live any more; discover it again"
    return type(error).__name__ + ': ' + str(error)[:200]
def get(reference, what):
    try: return bridge.refs.get(reference)
    except Exception as error: raise LookupError(what + ': ' + why(error))
def described(p):
    if p is None: return None
    device = getattr(p, 'canonical_parent', None)
    return {'name': str(p.name), 'device': str(getattr(device, 'name', '') or ''), 'identity': getattr(p, '_live_ptr', None)}
modulator = get(ARGS['device'], 'the modulator')
name = str(getattr(modulator, 'name', '') or 'That device')
# Willington's DeviceTools give Live's LFO, Shaper, Envelope Follower and Expression Control (Max devices) these.
if not callable(getattr(modulator, 'map_modulation', None)) or not callable(getattr(modulator, 'get_modulation_target', None)):
    if str(getattr(modulator, 'class_name', '')).startswith('MxDevice'):
        raise ValueError(name + " can't be mapped here: Live's modulators map through Willington's DeviceTools, which this Live hasn't loaded")
    raise ValueError(name + " isn't a modulator Kumi can map: an LFO, Shaper, Envelope Follower or Expression Control")
slot = int(ARGS['slot'])
if ARGS.get('clear'):
    target = None
elif ARGS.get('parameterRef'):
    target = get(ARGS['parameterRef'], 'the parameter')
else:
    device = get(ARGS['target'], 'the device to modulate')
    wanted = str(ARGS['parameter']).strip().lower()
    parameters = list(device.parameters)
    found = [p for p in parameters if str(p.name).strip().lower() == wanted] or [p for p in parameters if str(getattr(p, 'original_name', '')).strip().lower() == wanted]
    if not found:
        raise LookupError(str(device.name) + ' has no parameter called ' + str(ARGS['parameter']) + ': it has ' + ', '.join(str(p.name) for p in parameters[:48]))
    target = found[0]
# A slot past the modulator's own raises here, before anything changes.
prior = modulator.get_modulation_target(slot)
modulator.map_modulation(slot, target)
def read(what, otherwise):
    # A read once Live has changed: it may not fail, or the change would read as refused.
    try: return what()
    except Exception: return otherwise
now = read(lambda: modulator.get_modulation_target(slot), target)
track = modulator
while track is not None and type(track).__name__ != 'Track': track = read(lambda: getattr(track, 'canonical_parent', None), None)
result = {'modulator': name, 'slot': slot, 'prior': read(lambda: described(prior), None), 'now': read(lambda: described(now), None), 'track': track}
