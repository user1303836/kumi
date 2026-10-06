"""Resolve bundled modulators using real Live BrowserItems."""
MODULATORS = (
    ('LFO', 'audio_effects', 'query:AudioFx#LFO'),
    ('Shaper', 'audio_effects', 'query:AudioFx#Shaper'),
    ('Envelope Follower', 'audio_effects', 'query:AudioFx#Envelope%20Follower'),
    ('Expression Control', 'midi_effects', 'query:MidiFx#Expression%20Control'),
)


def get_modulators(browser):
    """Return available bundled modulators, in stable order, without loading them.

    Uses a native category if the host exposes a populated one. Otherwise resolves
    the known bundled devices by URI, avoiding similarly named user presets.
    BrowserItems are looked up afresh after browser refreshes.
    """
    category = getattr(browser, 'modulators', None)
    if category is not None:
        children = tuple(category.children)
        if children:
            return children
    found = []
    for name, category_name, uri in MODULATORS:
        items = getattr(browser, category_name).children
        matches = [item for item in items if item.uri == uri and item.is_loadable]
        if len(matches) > 1:
            raise RuntimeError('Ambiguous bundled modulator: ' + name)
        found.extend(matches)
    return tuple(found)
