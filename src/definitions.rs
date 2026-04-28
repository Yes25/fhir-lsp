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

/// Returns the defs lookup prefix to use when accessing children of `path`.
///
/// This is the single source of truth for type resolution across hover and
/// diagnostics. It handles three cases recursively:
///
/// - **Direct** (`"Patient.name"` → type `HumanName`): return `"HumanName"`.
/// - **Backbone** (`"MedicationRequest.dispenseRequest"` → `BackboneElement`):
///   return the full path, because children live under it in defs.
/// - **Choice type** (`"Medication.ingredient.itemCodeableConcept"`): resolve
///   the parent prefix, find the `item[x]` entry, extract the concrete type
///   suffix `"CodeableConcept"`, and return it.
///
/// Returns `None` for primitives, multi-type fields, or unrecognised paths.
pub fn child_lookup_prefix(path: &str, defs: &HashMap<String, ElementInfo>) -> Option<String> {
    // Direct lookup covers resource-level fields, backbone elements, and
    // single-type complex fields (HumanName, Period, …).
    if let Some(info) = defs.get(path) {
        return classify_child_prefix(info, path);
    }

    // Recursive: resolve through type references and choice types.
    let dot = path.rfind('.')?;
    let parent_path = &path[..dot];
    let field = &path[dot + 1..];

    let parent_prefix = child_lookup_prefix(parent_path, defs)?;

    // Regular field under the resolved parent prefix.
    let full_field_path = format!("{parent_prefix}.{field}");
    if let Some(info) = defs.get(&full_field_path) {
        return classify_child_prefix(info, &full_field_path);
    }

    // Choice type field under the resolved parent prefix.
    let (_, type_name) = find_choice_type(&parent_prefix, field, defs)?;
    if is_fhir_primitive(&type_name) || type_name == "Resource" {
        None
    } else {
        Some(type_name)
    }
}

/// Returns what prefix to use for looking up children of a field, given its
/// [`ElementInfo`] and the path it was found at.
fn classify_child_prefix(info: &ElementInfo, path: &str) -> Option<String> {
    match info.types.as_slice() {
        [t] if t == "BackboneElement" => Some(path.to_owned()),
        [t] if !is_fhir_primitive(t) && t != "Resource" => Some(t.to_owned()),
        _ => None,
    }
}

/// Resolves a FHIR element path to its [`ElementInfo`], following type
/// references and choice types at every level.
///
/// ```text
/// "Patient.name.family"          → HumanName.family  (via type reference)
/// "Patient.name.period.start"    → Period.start       (two hops)
/// "Medication.ingredient.itemCodeableConcept"
///                                → item[x] ElementInfo (choice type)
/// "Medication.ingredient.itemCodeableConcept.coding"
///                                → CodeableConcept.coding (choice type + child)
/// ```
pub fn resolve_path<'a>(
    path: &str,
    defs: &'a HashMap<String, ElementInfo>,
) -> Option<&'a ElementInfo> {
    if let Some(info) = defs.get(path) {
        return Some(info);
    }

    let dot = path.rfind('.')?;
    let parent_path = &path[..dot];
    let child = &path[dot + 1..];

    // Use child_lookup_prefix to determine where children of parent_path live.
    let child_prefix = child_lookup_prefix(parent_path, defs)?;

    if let Some(info) = defs.get(&format!("{child_prefix}.{child}")) {
        return Some(info);
    }

    find_choice_type(&child_prefix, child, defs).map(|(info, _)| info)
}

/// Searches `defs` for a choice-type entry (`{prefix}.{base}[x]`) whose base
/// is a prefix of `concrete_field` followed immediately by a PascalCase type
/// suffix.
///
/// Returns the matching [`ElementInfo`] and the concrete type name as an owned
/// `String` (e.g. `"CodeableConcept"` for concrete field `"itemCodeableConcept"`).
pub fn find_choice_type<'a>(
    prefix: &str,
    concrete_field: &str,
    defs: &'a HashMap<String, ElementInfo>,
) -> Option<(&'a ElementInfo, String)> {
    let full_prefix = format!("{prefix}.");
    for (key, info) in defs {
        if !key.starts_with(&full_prefix) {
            continue;
        }
        let rest = &key[full_prefix.len()..];
        if !rest.ends_with("[x]") || rest.contains('.') {
            continue;
        }
        let base = &rest[..rest.len() - 3];
        if let Some(suffix) = concrete_field.strip_prefix(base) {
            if suffix.starts_with(|c: char| c.is_uppercase()) {
                return Some((info, suffix.to_owned()));
            }
        }
    }
    None
}

/// Returns `true` for FHIR primitive type names that have no sub-fields.
pub fn is_fhir_primitive(t: &str) -> bool {
    matches!(
        t,
        "string"
            | "code"
            | "boolean"
            | "integer"
            | "decimal"
            | "uri"
            | "url"
            | "canonical"
            | "base64Binary"
            | "instant"
            | "date"
            | "dateTime"
            | "time"
            | "oid"
            | "id"
            | "markdown"
            | "unsignedInt"
            | "positiveInt"
            | "uuid"
            | "xhtml"
    )
}
