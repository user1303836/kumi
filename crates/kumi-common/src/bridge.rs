//! What Kumi's bridge says that Kumi itself acts on.

/// What the bridge says when Live is running another version of Kumi's Remote Script than the bridge's own. Live
/// loads it when it starts, so a restart takes the one installed. Starting Kumi again is the way out when this
/// bridge is the old one (a session left open through an update), and `kumi bridge` when another Kumi's Remote
/// Script sits in the same User Library.
pub const ANOTHER_BRIDGE: &str = "Live is running another version of Kumi's bridge than the one installed (Live loads it when it starts): restart Live. If Kumi was updated while it ran, start Kumi again; if it still says this, quit Live and run kumi bridge";
