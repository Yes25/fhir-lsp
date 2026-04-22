mod ast;
mod definitions;
mod fhir_index;

use definitions::{ElementInfo, FhirVersion};
use fhir_index::FhirIndex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::{Mutex, RwLock};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};
use tree_sitter::{Parser, Point, Tree};

struct DocumentState {
    text: String,
    tree: Tree,
    index: FhirIndex,
}

struct Backend {
    client: Client,
    parser: Mutex<Parser>,
    documents: RwLock<HashMap<Uri, DocumentState>>,
    workspace_folders: RwLock<Vec<PathBuf>>,
    fhir_version: RwLock<FhirVersion>,
}

impl Backend {
    fn new(client: Client) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_fhir_json::LANGUAGE.into())
            .expect("failed to load FHIR JSON grammar");
        Backend {
            client,
            parser: Mutex::new(parser),
            documents: RwLock::new(HashMap::new()),
            workspace_folders: RwLock::new(Vec::new()),
            fhir_version: RwLock::new(FhirVersion::default()),
        }
    }

    async fn parse_and_store(&self, uri: Uri, text: String) {
        let tree = self.parser.lock().await.parse(&text, None);
        match tree {
            Some(tree) => {
                let index = FhirIndex::build(&tree, &text);
                self.documents
                    .write()
                    .await
                    .insert(uri, DocumentState { text, tree, index });
            }
            None => {
                self.client
                    .log_message(MessageType::ERROR, "Failed to parse document")
                    .await;
            }
        }
    }

    /// Reads `uri` from disk and re-indexes it. If the file is gone, removes
    /// it from the index.
    async fn index_from_disk(&self, uri: Uri) {
        let Some(path) = uri.to_file_path() else {
            return;
        };
        match std::fs::read_to_string(&*path) {
            Ok(text) => self.parse_and_store(uri, text).await,
            Err(_) => {
                self.documents.write().await.remove(&uri);
            }
        }
    }

    /// Recursively collects all `.json` files under `dir`.
    fn collect_json_files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return files;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(Self::collect_json_files(&path));
            } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
                files.push(path);
            }
        }
        files
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store workspace folders so `initialized` can scan them.
        let folders: Vec<PathBuf> = params
            .workspace_folders
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.uri.to_file_path().map(|p| p.into_owned()))
            .collect();
        *self.workspace_folders.write().await = folders;

        // Read the initial FHIR version from initializationOptions if provided.
        // Clients send this as e.g. { "fhirVersion": "R5" }.
        if let Some(version) = params
            .initialization_options
            .as_ref()
            .and_then(|o| o.get("fhirVersion"))
            .and_then(|v| v.as_str())
            .and_then(FhirVersion::from_str)
        {
            *self.fhir_version.write().await = version;
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Register a file watcher so the server is notified when JSON files
        // change on disk outside the editor.
        self.client
            .register_capability(vec![Registration {
                id: "watch-fhir-json".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.json".to_string()),
                            kind: None,
                        }],
                    })
                    .unwrap(),
                ),
            }])
            .await
            .ok();

        // Index every JSON file in all workspace folders up front so that
        // cross-file goto-definition and goto-references work immediately.
        let folders = self.workspace_folders.read().await.clone();
        for folder in &folders {
            for path in Self::collect_json_files(folder) {
                if let Some(uri) = Uri::from_file_path(&path) {
                    self.index_from_disk(uri).await;
                }
            }
        }

        let version = self.fhir_version.read().await.as_str();
        self.client
            .show_message(MessageType::INFO, format!("fhir-lsp initialized (FHIR {version})"))
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        // The editor's live content takes priority over the on-disk version
        // we indexed at startup.
        self.parse_and_store(params.text_document.uri, params.text_document.text)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // With FULL sync there is always exactly one entry with the complete text.
        if let Some(change) = params.content_changes.into_iter().last() {
            self.parse_and_store(params.text_document.uri, change.text)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Revert to the on-disk version so the file stays in the index and
        // cross-file references continue to resolve correctly.
        self.index_from_disk(params.text_document.uri).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            match change.typ {
                FileChangeType::DELETED => {
                    self.documents.write().await.remove(&change.uri);
                }
                // CREATED or CHANGED on disk — re-read and re-index.
                _ => self.index_from_disk(change.uri).await,
            }
        }
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // Clients differ in how they nest settings.  Try the flat form first
        // ({ "fhirVersion": "R5" }), then the VS Code namespaced form
        // ({ "fhir-lsp": { "fhirVersion": "R5" } }).
        let version_str = params
            .settings
            .get("fhirVersion")
            .or_else(|| params.settings.get("fhir-lsp")?.get("fhirVersion"))
            .and_then(|v| v.as_str());

        let Some(s) = version_str else {
            return;
        };

        match FhirVersion::from_str(s) {
            Some(version) => {
                *self.fhir_version.write().await = version;
                self.client
                    .show_message(
                        MessageType::INFO,
                        format!("fhir-lsp: FHIR version changed to {}", version.as_str()),
                    )
                    .await;
            }
            None => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("fhir-lsp: unknown FHIR version {s:?}, ignoring"),
                    )
                    .await;
            }
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let documents = self.documents.read().await;
        let Some(doc) = documents.get(&uri) else {
            return Ok(None);
        };

        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };
        let Some(node) = doc
            .tree
            .root_node()
            .descendant_for_point_range(point, point)
        else {
            return Ok(None);
        };

        // Only makes sense when the cursor is on a `"reference"` value.
        let Some(ref_key) = doc.index.reference_at(node, &doc.text) else {
            return Ok(None);
        };

        // Search all open documents for the definition.
        for (def_uri, def_doc) in documents.iter() {
            if let Some(&range) = def_doc.index.definitions.get(&ref_key) {
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: def_uri.clone(),
                    range,
                })));
            }
        }

        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let documents = self.documents.read().await;
        let Some(doc) = documents.get(&uri) else {
            return Ok(None);
        };

        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };
        let Some(node) = doc
            .tree
            .root_node()
            .descendant_for_point_range(point, point)
        else {
            return Ok(None);
        };

        // Accept the cursor on either a `"reference"` value (look up who else
        // points to the same target) or on a resource's own id/type field
        // (look up all references to this resource).
        let Some(key) = doc
            .index
            .reference_at(node, &doc.text)
            .or_else(|| doc.index.definition_key_at(node, &doc.text))
        else {
            return Ok(None);
        };

        // Collect matching reference ranges across all open documents.
        let mut locations: Vec<Location> = Vec::new();
        for (ref_uri, ref_doc) in documents.iter() {
            if let Some(ranges) = ref_doc.index.references.get(&key) {
                for &range in ranges {
                    locations.push(Location {
                        uri: ref_uri.clone(),
                        range,
                    });
                }
            }
        }

        if locations.is_empty() {
            Ok(None)
        } else {
            Ok(Some(locations))
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let documents = self.documents.read().await;
        let Some(doc) = documents.get(&uri) else {
            return Ok(None);
        };

        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };
        let Some(node) = doc
            .tree
            .root_node()
            .descendant_for_point_range(point, point)
        else {
            return Ok(None);
        };

        let Some(path) = ast::fhir_path_at(node, &doc.text) else {
            return Ok(None);
        };

        let defs = definitions::for_version(*self.fhir_version.read().await);
        let Some(info) = defs.get(&path) else {
            return Ok(None);
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: render_hover(&path, info),
            }),
            range: None,
        }))
    }
}

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
fn render_hover(path: &str, info: &ElementInfo) -> String {
    let mut md = String::new();

    // Signature line: type(s) and cardinality
    let types_str = info
        .types
        .iter()
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join(" | ");
    let cardinality = format!("`{}..{}`", info.min, info.max);

    if types_str.is_empty() {
        md.push_str(&format!("**{path}** · {cardinality}"));
    } else {
        md.push_str(&format!("**{path}** · {types_str} · {cardinality}"));
    }
    md.push_str("\n\n");

    // Short description
    if let Some(short) = &info.short {
        md.push_str(&format!("\n{short}\n\n"));
    }

    // Longer definition (only present when it differs from short)
    if let Some(definition) = &info.definition {
        md.push_str(&format!("**--- Definition ---**\n{definition}\n\n"));
    }

    // Field-specific constraints
    if !info.constraints.is_empty() {
        md.push_str("`Constraints`\n\n");
        for c in &info.constraints {
            md.push_str(&format!("- {c}\n"));
        }
    }

    md.trim_end().to_owned()
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
