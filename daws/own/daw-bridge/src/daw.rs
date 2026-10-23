//! The DAWs these bridges serve, and the ports each one's bridge listens on.
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

/// Ableton's bridge ports (the Remote Script's and the realtime one): never used by another DAW's bridge.
pub const ABLETON_PORTS: [u16; 2] = [9765, 9766];

/// How many ports each DAW's range holds: one per running instance, first free one wins.
pub const PORTS_PER_DAW: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Daw {
    Reaper,
    Renoise,
}

impl Daw {
    pub const ALL: [Daw; 2] = [Daw::Reaper, Daw::Renoise];

    /// The id on the wire, in file names and on the command line.
    pub fn id(self) -> &'static str {
        match self {
            Daw::Reaper => "reaper",
            Daw::Renoise => "renoise",
        }
    }

    pub fn from_id(id: &str) -> Option<Daw> {
        Daw::ALL.into_iter().find(|daw| daw.id() == id)
    }

    /// The app's name as people write it.
    pub fn name(self) -> &'static str {
        match self {
            Daw::Reaper => "REAPER",
            Daw::Renoise => "Renoise",
        }
    }

    /// The first port the bridge tries when its reference file names none.
    pub fn default_port(self) -> u16 {
        match self {
            Daw::Reaper => 9770,
            Daw::Renoise => 9780,
        }
    }

    /// Every port this DAW's bridge may listen on.
    pub fn ports(self) -> RangeInclusive<u16> {
        let start = self.default_port();
        start..=start + PORTS_PER_DAW - 1
    }
}

impl std::fmt::Display for Daw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_ranges_never_overlap_each_other_or_ableton() {
        for daw in Daw::ALL {
            assert!(daw.ports().contains(&daw.default_port()));
            assert!(ABLETON_PORTS.iter().all(|port| !daw.ports().contains(port)), "{daw}");
            for other in Daw::ALL.into_iter().filter(|other| *other != daw) {
                assert!(daw.ports().all(|port| !other.ports().contains(&port)), "{daw} and {other}");
            }
            assert_eq!(Daw::from_id(daw.id()), Some(daw));
            assert_eq!(serde_json::to_value(daw).unwrap(), daw.id());
        }
    }
}
