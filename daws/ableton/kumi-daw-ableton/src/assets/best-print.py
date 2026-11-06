import hashlib

# Moves while the Set plays or Live thinks, with nobody editing anything.
VOLATILE = {
    "is_playing", "is_triggered", "is_recording", "is_overdubbing", "playing_position", "playing_position_enabled", "will_record_on_start",
    "latency_in_ms", "latency_in_samples", "is_active", "is_showing_chains", "selected_variation_index",
}


def plain(item):
    """What a Live object says of itself in plain values: state no parameter shows, as a Wavetable's wavetable."""
    row = {}
    for name in dir(item):
        if name.startswith("_") or name in VOLATILE:
            continue
        # Looper's state follows playback; another object's state may be an edit worth keeping.
        if name == "state" and getattr(item, "class_name", None) == "Looper":
            continue
        try:
            value = getattr(item, name)
        except Exception:
            continue
        if isinstance(value, float):
            row[name] = round(value, 6)
        elif isinstance(value, (bool, int, str)):
            row[name] = value
    return row


def knob(parameter):
    return [parameter.name, round(float(parameter.value), 6), str(parameter.automation_state)]


def device_row(device, parameters):
    parameters.extend(device.parameters)
    row = plain(device)
    row["parameters"] = [knob(parameter) for parameter in device.parameters]
    sample = getattr(device, "sample", None)
    if sample is not None:
        row["sample"] = plain(sample)
    for name in ("chains", "return_chains"):
        chains = getattr(device, name, None)
        if chains:
            row[name] = [dict(plain(chain), devices=[device_row(inner, parameters) for inner in chain.devices]) for chain in chains]
    return row


def clip_row(clip, parameters):
    row = plain(clip)
    if clip.is_midi_clip:
        row["notes"] = sorted(
            [note.pitch, note.start_time, note.duration, note.velocity, note.mute, note.probability, note.velocity_deviation, note.release_velocity]
            for note in clip.get_all_notes_extended()
        )
    else:
        row["warp"] = [[marker.beat_time, marker.sample_time] for marker in getattr(clip, "warp_markers", [])]
    if clip.has_envelopes:
        envelopes = []
        for parameter in parameters:
            try:
                envelope = clip.automation_envelope(parameter)
            except Exception:
                continue
            if envelope is not None:
                envelopes.append([parameter.name, [round(envelope.value_at_time(clip.length * at / 64), 6) for at in range(65)]])
        row["envelopes"] = envelopes
    return row


def mixer_row(track):
    mixer = track.mixer_device
    routing = (track.input_routing_type, track.input_routing_channel, track.output_routing_type, track.output_routing_channel)
    return {
        # (not its mute or solo: a paused best's notice asks for it to be unmuted)
        "knobs": [knob(parameter) for parameter in [mixer.volume, mixer.panning] + list(mixer.sends)],
        "routing": [choice.display_name for choice in routing],
    }


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, default=str).encode("utf-8")).hexdigest()


tracks = [track for track in song.tracks if track.name == ARGS["track"]]
if len(tracks) > 1:
    raise ValueError("Two tracks are named " + ARGS["track"])
if tracks:
    track = tracks[0]
    parameters = [track.mixer_device.volume, track.mixer_device.panning] + list(track.mixer_device.sends)
    devices = [device_row(device, parameters) for device in track.devices]
    clips = [
        [clip_row(slot.clip, parameters) for slot in track.clip_slots if slot.has_clip],
        [clip_row(clip, parameters) for clip in track.arrangement_clips],
        [[clip_row(clip, parameters) for clip in lane.arrangement_clips] for lane in getattr(track, "take_lanes", [])],
    ]
    result = {"devices": digest(devices), "clips": digest(clips), "mixer": digest(mixer_row(track))}
