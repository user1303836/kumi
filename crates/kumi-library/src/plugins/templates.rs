//! Initialized VST3 states captured from the installed plugins. No factory sounds.

pub fn initial(plugin: &str) -> Option<&'static [u8]> {
    match plugin {
        "serum-2" => Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.vstpreset"))),
        "vital" => Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/vital/format-1/fixtures/init.vstpreset"))),
        "minimal-cluster-delay" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-cluster-delay/format-1/fixtures/init.vstpreset")))
        }
        "minimal-current" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-current/format-1/fixtures/init.vstpreset")))
        }
        "minimal-eprom-memory-rites" => Some(include_bytes!(concat!(
            env!("KUMI_WORKSPACE"),
            "/plugin-formats/minimal-eprom-memory-rites/format-1/fixtures/init.vstpreset"
        ))),
        "minimal-evoke" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-evoke/format-1/fixtures/init.vstpreset")))
        }
        "minimal-flex-chorus" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-flex-chorus/format-1/fixtures/init.vstpreset")))
        }
        "minimal-formant" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-formant/format-1/fixtures/init.vstpreset")))
        }
        "minimal-fuse-compressor" => Some(include_bytes!(concat!(
            env!("KUMI_WORKSPACE"),
            "/plugin-formats/minimal-fuse-compressor/format-1/fixtures/init.vstpreset"
        ))),
        "minimal-hybrid-filter" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-hybrid-filter/format-1/fixtures/init.vstpreset")))
        }
        "minimal-lucid" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-lucid/format-1/fixtures/init.vstpreset")))
        }
        "minimal-morph-eq" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-morph-eq/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p11-abyss" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p11-abyss/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p19-igloo" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p19-igloo/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p42-climax" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p42-climax/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p422-fairuz" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p422-fairuz/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p44-magnum" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p44-magnum/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p440-sweet-spot" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p440-sweet-spot/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p450-mdn-eq" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p450-mdn-eq/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p455-mdn-sidecar" => Some(include_bytes!(concat!(
            env!("KUMI_WORKSPACE"),
            "/plugin-formats/pulsar-p455-mdn-sidecar/format-1/fixtures/init.vstpreset"
        ))),
        "pulsar-p565-siren" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p565-siren/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p821-mdn-tape" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p821-mdn-tape/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p915-medusa" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-p915-medusa/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-p930-lunar-lander" => Some(include_bytes!(concat!(
            env!("KUMI_WORKSPACE"),
            "/plugin-formats/pulsar-p930-lunar-lander/format-1/fixtures/init.vstpreset"
        ))),
        "minimal-poly-flanger" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-poly-flanger/format-1/fixtures/init.vstpreset")))
        }
        "pulsar-q-sahara" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/pulsar-q-sahara/format-1/fixtures/init.vstpreset")))
        }
        "minimal-rift" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-rift/format-1/fixtures/init.vstpreset")))
        }
        "minimal-ripple-phaser" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-ripple-phaser/format-1/fixtures/init.vstpreset")))
        }
        "minimal-squash" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-squash/format-1/fixtures/init.vstpreset")))
        }
        "minimal-swarm-reverb" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-swarm-reverb/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-delay" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-delay/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-freq-echo" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-freq-echo/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-future-verb" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-future-verb/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-plate" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-plate/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-room" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-room/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-shimmer" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-shimmer/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-space-modulator" => Some(include_bytes!(concat!(
            env!("KUMI_WORKSPACE"),
            "/plugin-formats/valhalla-space-modulator/format-1/fixtures/init.vstpreset"
        ))),
        "valhalla-supermassive" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-supermassive/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-uber-mod" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-uber-mod/format-1/fixtures/init.vstpreset")))
        }
        "valhalla-vintage-verb" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/valhalla-vintage-verb/format-1/fixtures/init.vstpreset")))
        }
        "minimal-wave-shifter" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/minimal-wave-shifter/format-1/fixtures/init.vstpreset")))
        }
        "zynaptiq-punch-1" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/zynaptiq-punch-1/format-1/fixtures/init.vstpreset")))
        }
        "zynaptiq-unmix-drums" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/zynaptiq-unmix-drums/format-1/fixtures/init.vstpreset")))
        }
        "superior-drummer-3" => {
            Some(include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/superior-drummer-3/format-1/fixtures/empty.vstpreset")))
        }
        _ => None,
    }
}
