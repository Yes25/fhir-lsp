#!/usr/bin/env python3
"""Extract hover-relevant ElementDefinition data from FHIR definition bundles.

Reads profiles-resources.json and profiles-types.json for each FHIR version
and writes a compact JSON file keyed by element path:

    {
      "Patient.name": {
        "min": 0,
        "max": "*",
        "types": ["HumanName"],
        "short": "A name associated with the patient",
        "definition": "A name associated with the individual.",
        "constraints": ["SHALL have at least a family or given name"]
      },
      ...
    }

Output files are written to assets/ next to this script's parent directory.

Usage:
    python3 scripts/extract_fhir_definitions.py
"""

from __future__ import annotations

import json
from pathlib import Path

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).parent.parent

VERSIONS: list[dict] = [
    {
        "label": "R4",
        "dir": REPO_ROOT / "definitions_4_0_1",
        "output": REPO_ROOT / "assets" / "fhir_r4.json",
    },
    {
        "label": "R4B",
        # directory was created with a .json suffix by accident
        "dir": REPO_ROOT / "definitions_4_3_0.json",
        "output": REPO_ROOT / "assets" / "fhir_r4b.json",
    },
    {
        "label": "R5",
        "dir": REPO_ROOT / "definitions_5_0_0",
        "output": REPO_ROOT / "assets" / "fhir_r5.json",
    },
]

# Source files that contain StructureDefinitions for resources and data types.
# profiles-others.json also has SDs but they are profiled resources (e.g.
# domain-specific profiles) whose paths clash with the base resources, so we
# skip it to keep the output unambiguous.
SD_FILES = ["profiles-resources.json", "profiles-types.json"]

# Constraint key to suppress on every element — it carries no field-specific
# information and would clutter every hover tooltip.
NOISE_CONSTRAINT_KEYS = {"ele-1"}

# StructureDefinition kinds to skip (logical models are not concrete resources)
SKIP_KINDS = {"logical"}

FHIRPATH_TYPE_EXT = (
    "http://hl7.org/fhir/StructureDefinition/structuredefinition-fhir-type"
)


# ---------------------------------------------------------------------------
# Extraction helpers
# ---------------------------------------------------------------------------


def normalise_type_code(type_entry: dict) -> str:
    """Return a human-readable type code for a single type[] entry.

    FHIR uses `http://hl7.org/fhirpath/System.String` (and siblings) for
    primitive types.  The real FHIR name is carried in the
    `structuredefinition-fhir-type` extension.
    """
    code: str = type_entry.get("code", "")
    if "fhirpath" in code:
        for ext in type_entry.get("extension", []):
            if ext.get("url") == FHIRPATH_TYPE_EXT:
                return ext.get("valueUrl", code)
    return code


def extract_types(element: dict) -> list[str]:
    return [normalise_type_code(t) for t in element.get("type", [])]


def extract_constraints(element: dict) -> list[str]:
    return [
        c["human"]
        for c in element.get("constraint", [])
        if c.get("key") not in NOISE_CONSTRAINT_KEYS and c.get("human")
    ]


def extract_elements(sd: dict) -> dict[str, dict]:
    """Extract all ElementDefinitions from a single StructureDefinition."""
    elements: dict[str, dict] = {}

    snapshot = sd.get("snapshot", {})
    for el in snapshot.get("element", []):
        path: str = el.get("path", "")

        # Skip the root element (e.g. bare "Patient" or "HumanName") — it has
        # no field-level information relevant to hover on a JSON key.
        if "." not in path:
            continue

        types = extract_types(el)
        constraints = extract_constraints(el)

        entry: dict = {
            "min": el.get("min"),
            "max": el.get("max"),
        }
        if types:
            entry["types"] = types
        if el.get("short"):
            entry["short"] = el["short"]
        if el.get("definition") and el["definition"] != el.get("short"):
            entry["definition"] = el["definition"]
        if constraints:
            entry["constraints"] = constraints

        elements[path] = entry

    return elements


def extract_from_bundle(bundle_path: Path) -> dict[str, dict]:
    """Extract all ElementDefinitions from every StructureDefinition in a Bundle."""
    with open(bundle_path, encoding="utf-8") as f:
        bundle = json.load(f)

    all_elements: dict[str, dict] = {}
    sd_count = 0

    for entry in bundle.get("entry", []):
        resource = entry.get("resource", {})
        if resource.get("resourceType") != "StructureDefinition":
            continue
        if resource.get("kind") in SKIP_KINDS:
            continue

        elements = extract_elements(resource)
        all_elements.update(elements)
        sd_count += 1

    print(
        f"    {bundle_path.name}: {sd_count} StructureDefinitions, "
        f"{len(all_elements)} elements"
    )
    return all_elements


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> None:
    assets_dir = REPO_ROOT / "assets"
    assets_dir.mkdir(exist_ok=True)

    for version in VERSIONS:
        label = version["label"]
        source_dir: Path = version["dir"]
        output_path: Path = version["output"]

        if not source_dir.exists():
            print(f"[{label}] Source directory not found: {source_dir} — skipping")
            continue

        print(f"[{label}] Reading from {source_dir.name}/")
        all_elements: dict[str, dict] = {}

        for filename in SD_FILES:
            source_file = source_dir / filename
            if not source_file.exists():
                print(f"    {filename}: not found — skipping")
                continue
            elements = extract_from_bundle(source_file)
            all_elements.update(elements)

        with open(output_path, "w", encoding="utf-8") as f:
            json.dump(all_elements, f, ensure_ascii=False, separators=(",", ":"))

        size_kb = output_path.stat().st_size / 1024
        print(
            f"  → {output_path.relative_to(REPO_ROOT)}: "
            f"{len(all_elements)} paths, {size_kb:.0f} KB"
        )


if __name__ == "__main__":
    main()
