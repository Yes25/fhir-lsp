//! Embedded FHIR element definitions for hover documentation.
//!
//! The compressed JSON assets are baked into the binary at compile time.
//! Each version's map is decompressed and deserialized once on first access,
//! then cached for the lifetime of the process.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

// ── Embedded assets ──────────────────────────────────────────────────────────

static R4_COMPRESSED: &[u8] = include_bytes!("../assets/fhir_r4.json.zst");
static R4B_COMPRESSED: &[u8] = include_bytes!("../assets/fhir_r4b.json.zst");
static R5_COMPRESSED: &[u8] = include_bytes!("../assets/fhir_r5.json.zst");

static R4_DEFS: LazyLock<HashMap<String, ElementInfo>> = LazyLock::new(|| load(R4_COMPRESSED));
static R4B_DEFS: LazyLock<HashMap<String, ElementInfo>> = LazyLock::new(|| load(R4B_COMPRESSED));
static R5_DEFS: LazyLock<HashMap<String, ElementInfo>> = LazyLock::new(|| load(R5_COMPRESSED));

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FhirVersion {
    #[default]
    R4,
    R4B,
    R5,
}

impl FhirVersion {
    /// Parse a version string, accepting common shorthands case-insensitively.
    ///
    /// Accepted values:
    /// - `"R4"`, `"4.0"`, `"4.0.1"` → [`FhirVersion::R4`]
    /// - `"R4B"`, `"4.3"`, `"4.3.0"` → [`FhirVersion::R4B`]
    /// - `"R5"`, `"5.0"`, `"5.0.0"` → [`FhirVersion::R5`]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "R4" | "4.0" | "4.0.1" => Some(Self::R4),
            "R4B" | "4.3" | "4.3.0" => Some(Self::R4B),
            "R5" | "5.0" | "5.0.0" => Some(Self::R5),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::R4 => "R4",
            Self::R4B => "R4B",
            Self::R5 => "R5",
        }
    }
}

/// The hover-relevant fields extracted from a FHIR ElementDefinition.
#[derive(Debug, Deserialize)]
pub struct ElementInfo {
    pub min: u32,
    pub max: String,
    #[serde(default)]
    pub types: Vec<String>,
    pub short: Option<String>,
    pub definition: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
}

impl ElementInfo {
    /// Renders an [`ElementInfo`] as a Markdown hover string.
    ///
    /// Format:
    /// ```text
    /// `HumanName` · `0..*`
    ///
    /// A name associated with the patient.
    ///
    /// A name associated with the individual.
    ///
    /// **Constraints**
    /// - SHALL have at least a family or given name
    /// ```
    pub fn render_hover(&self, path: &str) -> String {
        let mut md = String::new();

        // Signature line: type(s) and cardinality
        let types_str = self
            .types
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(" | ");
        let cardinality = format!("`{}..{}`", self.min, self.max);

        if types_str.is_empty() {
            md.push_str(&format!("**{path}** · {cardinality}"));
        } else {
            md.push_str(&format!("**{path}** · {types_str} · {cardinality}"));
        }
        md.push_str("\n\n");

        // Short description
        if let Some(short) = &self.short {
            md.push_str(&format!("\n{short}\n\n"));
        }

        // Longer definition (only present when it differs from short)
        if let Some(definition) = &self.definition {
            md.push_str(&format!("**Definition:**\n{definition}\n\n"));
        }

        // Field-specific constraints
        if !self.constraints.is_empty() {
            md.push_str("`Constraints`\n\n");
            for c in &self.constraints {
                md.push_str(&format!("- {c}\n"));
            }
        }

        md.trim_end().to_owned()
    }
}

// ── Loading ──────────────────────────────────────────────────────────────────

fn load(compressed: &[u8]) -> HashMap<String, ElementInfo> {
    let json =
        zstd::decode_all(compressed).expect("failed to decompress embedded FHIR definitions");
    serde_json::from_slice(&json).expect("failed to deserialize embedded FHIR definitions")
}

/// Returns the element definition map for the given FHIR version.
///
/// The first call for a given version decompresses and deserializes the
/// embedded asset; subsequent calls return the cached map immediately.
pub fn for_version(version: FhirVersion) -> &'static HashMap<String, ElementInfo> {
    match version {
        FhirVersion::R4 => &R4_DEFS,
        FhirVersion::R4B => &R4B_DEFS,
        FhirVersion::R5 => &R5_DEFS,
    }
}
