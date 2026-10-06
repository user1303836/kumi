"""Mapping adapters for the fingerprinted bundled Max modulators."""
import json
import os
import Live
import _MxDCore

with open(os.path.join(os.path.dirname(__file__), 'modulator_profiles.json')) as stream:
    PROFILES = json.load(stream)


def resolve(native, device):
    if not device:
        raise ValueError('Modulator has been deleted')
    signature = [[p.original_name, p.min, p.max, p.is_quantized] for p in device.parameters]
    matches = [p for p in PROFILES.values() if p['parameters'] == signature]
    if len(matches) != 1:
        raise ValueError('Unsupported modulator parameter signature')
    paths = matches[0]['slots']
    core = _MxDCore.MxDCoreCls.instance
    if core is None:
        raise RuntimeError('Max bridge is not initialized')
    contexts = [(did, context) for did, context in core.device_contexts.items()
                if core.manager.get_max_device(did) == device]
    if len(contexts) != 1:
        raise RuntimeError('Modulator has not finished loading')
    did, context = contexts[0]
    found = {}
    for oid in tuple(context):
        if not isinstance(oid, int) or oid <= 0:
            continue
        try:
            name = native.object_name(device, oid)
        except RuntimeError:
            continue  # Some transient Max objects do not have persistent names.
        if name in paths:
            if name in found:
                raise RuntimeError('Ambiguous modulator mapping slot')
            found[name] = oid
    if len(found) != len(paths):
        raise RuntimeError('Modulator mapping objects are not ready or have changed')
    return core, did, [found[path] for path in paths], paths


def map_parameter(native, device, slot, parameter):
    if not native.writes_enabled:
        raise RuntimeError('Willington device writes are disabled')
    if parameter is not None and not isinstance(parameter, Live.DeviceParameter.DeviceParameter):
        raise TypeError('Expected a Live DeviceParameter or None')
    if parameter is not None and not parameter:
        raise ValueError('Target parameter has been deleted')
    core, did, slots, paths = resolve(native, device)
    if type(slot) is not int or not 0 <= slot < len(slots):
        raise ValueError('Mapping slot is outside this modulator\'s range')
    oid = slots[slot]
    if parameter is not None:
        current = core._get_current_lom_id(did, oid)
        current = core.manager.get_lom_object(did, current) if current else None
        error = native.validate_modulation(parameter._live_ptr, current == parameter)
        if error:
            raise ValueError(error.decode('utf-8', 'replace'))
    lom_id = core.manager.get_lom_id(parameter) if parameter is not None else 0
    if parameter is None:
        prefix = paths[slot].removesuffix('::obj-16::obj-130')
        roles = {prefix+'::obj-10': core.mod_set_id,
                 prefix+'::obj-5': core.rmt_set_id,
                 prefix+'::obj-16::obj-5': core.obs_set_id}
        for candidate in tuple(core.device_contexts[did]):
            if not isinstance(candidate, int) or candidate <= 0:
                continue
            try:
                name = native.object_name(device, candidate)
            except RuntimeError:
                continue
            if name in roles:
                roles[name](did, candidate, '0')
    core.obj_set_id(did, oid, str(lom_id))
    core.obj_get_id(did, oid, '')
    core.obj_get_path(did, oid, '')


def mapped_parameter(native, device, slot):
    core, did, slots, paths = resolve(native, device)
    if type(slot) is not int or not 0 <= slot < len(slots):
        raise ValueError('Mapping slot is outside this modulator\'s range')
    lom_id = core._get_current_lom_id(did, slots[slot])
    return core.manager.get_lom_object(did, lom_id) if lom_id else None
