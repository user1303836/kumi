# PLACES are where a restructure moved refs since Live was last read there. Live's registry keeps objects by ref
# string, so under each it still holds what used to be at the place: read them again, as the bridge does before it
# acts at a place, so each ref names what is there now.
if not callable(getattr(bridge, '_refresh', None)): raise LookupError("Live's references changed since Kumi read them; discover again")
bridge._refresh(*PLACES)
