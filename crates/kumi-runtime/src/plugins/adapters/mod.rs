pub mod decapitator;
pub mod ott;
pub mod ozone12;
pub mod pigments;
pub mod prol2;
pub mod proq4;
pub mod saturn2;
pub mod serum2;
pub mod supermassive;
pub mod vital;

use std::sync::LazyLock;

use crate::plugins::adapter::PluginAdapter;

pub static ADAPTERS: LazyLock<Vec<&'static PluginAdapter>> = LazyLock::new(|| {
    vec![
        &*serum2::SERUM2,
        &*vital::VITAL,
        &*ozone12::OZONE12,
        &*proq4::PRO_Q4,
        &*prol2::PRO_L2,
        &*saturn2::SATURN2,
        &*ott::OTT,
        &*supermassive::SUPERMASSIVE,
        &*decapitator::DECAPITATOR,
        &*pigments::PIGMENTS,
    ]
});
