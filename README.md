# fhir-lsp

A Language Server Protocol (LSP) server for FHIR JSON resources. It provides real-time validation, documentation, and navigation for FHIR R4, R4B, and R5 files directly in your editor.

## Features

### Hover documentation

Hover over any field to see its FHIR element definition: type, cardinality, short description, full definition, and any constraints.

### Diagnostics

- Cardinality violations
- Missing required fields
- JSON syntax errors

### Go to definition

With the cursor on a `"reference"` value (e.g. `"Patient/123"`), jump to the resource within the same file (e.g. Bundle) or in any file in the workspace.

### Find references

With the cursor on a resource, list every `"reference"` field across the workspace that points to that resource.

### Formatting

Format the current document to produce a pretty-printed JSON. Fields within each resource are ordered to match the sequence they appear in the FHIR specification. `resourceType` is always placed first (needed for proper highlighting with Tree-Sitter).

## Configuration

The FHIR version `R4`, `R4B` and `R5` are supported. The Default is R4.
**Setup in Zed:**

```json
"lsp": {
      "fhir-lsp": {
        "initialization_options": {
          "fhirVersion": "R4B"
        }
      }
    }
```

**Neovim / editors that pass `initializationOptions`:**

```lua
require("lspconfig").fhir_lsp.setup({
 init_options = { fhirVersion = "R4" },
})
```

## Building from source

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- Python 3.8+ (for asset generation)
- `zstd` CLI (for compression): `brew install zstd`
- FHIR definition packages (downloaded separately — see below)

### 1. Download FHIR definitions

Download the official definition packages from [hl7.org](https://hl7.org/fhir/) and extract each one into a directory into this repository:

| Version | Package file              | Extract to              |
|---------|---------------------------|-------------------------|
| R4      | `definitions.json.zip`    | `definitions_4_0_1/`    |
| R4B     | `definitions.json.zip`    | `definitions_4_3_0.json/` |
| R5      | `definitions.json.zip`    | `definitions_5_0_0/`    |

Each extracted directory must contain `profiles-resources.json` and `profiles-types.json`.

### 2. Generate the definition assets

```bash
python3 scripts/extract_fhir_definitions.py
```

This reads the raw FHIR definitions, extracts the element metadata needed for hover and validation, and writes JSON files to `assets/`.

### 3. Compress the assets

```bash
bash scripts/compress_assets.sh
```

Compresses the JSON files to `.zst` format. These compressed assets are embedded in the binary at compile time.

### 4. Build

```bash
cargo build --release
```

The binary is written to `target/release/fhir-lsp`. It communicates over stdin/stdout and is ready to be registered as an LSP server in any editor.
