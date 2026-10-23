//! What sound matching reports: how close a render is to a reference (`Closeness`), feature by feature, and the gap no
//! knob closes (`Structural`). Measuring it is kumi-runtime's `audio::matching`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureName {
    Balance,
    Tilt,
    Brightness,
    Movement,
    Envelope,
    Pitch,
    Density,
    Width,
    Rhythm,
    Contour,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub name: FeatureName,
    /// 0 to 100.
    pub similarity: f64,
    /// What to change, when it's far enough off to matter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
    pub weight: f64,
}

/// What a comparison weighs for: a single sound, or a section of a mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Focus {
    Sound,
    Section,
}

/// The kind of gap no knob closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructuralKind {
    MissingLow,
    ExcessLow,
    MissingTop,
    ExcessTop,
    MissingMids,
    ExcessMids,
    Envelope,
    Register,
    Pitched,
    Width,
    Density,
}

/// A gap no knob closes (a band 9 dB or more off, an attack three times off, the wrong register, far
/// too wide or narrow), when it's the largest part of what's lost: what it is, and the structural
/// change that closes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Structural {
    pub kind: StructuralKind,
    pub gap: String,
    pub r#move: String,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Closeness {
    /// 0 to 100: the weighted mean of the features both sides have.
    pub score: f64,
    pub focus: Focus,
    pub features: Vec<Feature>,
    /// The biggest gaps, in words, biggest first.
    pub gaps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural: Option<Structural>,
}
