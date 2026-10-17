//! Semantic Rust documentation, opened outside the editor without building docs.
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use helix_lsp::{lsp, ExternalDocs};

use super::Context;
use crate::job::Callback;

pub fn open_documentation(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let Some(server) = doc
        .language_servers()
        .find(|server| server.name() == "rust-analyzer")
    else {
        cx.editor
            .set_error("Opening documentation requires rust-analyzer");
        return;
    };
    let position = doc.position(view.id, server.offset_encoding());
    let docs = server.external_docs(doc.identifier(), position);
    let definition = server.goto_definition(doc.identifier(), position, None);
    let source = doc.identifier().uri.to_file_path().ok();

    cx.jobs.callback(async move {
        let docs = docs.await?;
        if let Some(local) = existing_local(&docs).await {
            return crate::open_external_url_callback(local).await;
        }
        let web = match docs {
            ExternalDocs::Web(web) | ExternalDocs::WithLocal { web, .. } => web,
        }
        .filter(|url| matches!(url.scheme(), "https" | "http"));
        if let Some(web) = web {
            // A web URL alone does not prove the crate is published: RA invents
            // docs.rs URLs even for workspace crates. Resolve the definition's
            // package before permitting fallback. Never look up a word by name.
            let definition = match definition {
                Some(definition) => definition.await?,
                None => None,
            };
            if let (Some(source), Some(definition)) = (source, definition) {
                if !workspace_definition(&source, definition).await? {
                    return crate::open_external_url_callback(web).await;
                }
            }
        }
        Ok(Callback::Editor(Box::new(|editor| {
            editor.set_status("No documentation available (generate local docs with cargo doc)");
        })))
    });
}

async fn existing_local(docs: &ExternalDocs) -> Option<lsp::Url> {
    let ExternalDocs::WithLocal {
        local: Some(url), ..
    } = docs
    else {
        return None;
    };
    // The fork's lightweight URL type does not strip fragments when converting
    // file URLs. Check the HTML file, but send the full method anchor to the browser.
    let file = lsp::Url::parse(url.as_str().split(['#', '?']).next()?).ok()?;
    let path = file.to_file_path().ok()?;
    tokio::fs::metadata(path)
        .await
        .ok()?
        .is_file()
        .then(|| url.clone())
}

async fn manifest_for(source: &Path) -> Option<PathBuf> {
    for directory in source.parent()?.ancestors() {
        let manifest = directory.join("Cargo.toml");
        if tokio::fs::try_exists(&manifest).await.ok()? {
            return tokio::fs::canonicalize(manifest).await.ok();
        }
    }
    None
}

/// True also when provenance is unknown: fail closed rather than open a guessed URL.
async fn workspace_definition(
    source: &Path,
    definition: lsp::GotoDefinitionResponse,
) -> anyhow::Result<bool> {
    let uris: Vec<_> = match definition {
        lsp::GotoDefinitionResponse::Scalar(location) => vec![location.uri],
        lsp::GotoDefinitionResponse::Array(locations) => {
            locations.into_iter().map(|location| location.uri).collect()
        }
        lsp::GotoDefinitionResponse::Link(links) => {
            links.into_iter().map(|link| link.target_uri).collect()
        }
    };
    if uris.is_empty() {
        return Ok(true);
    }
    let Some(manifest) = manifest_for(source).await else {
        return Ok(true);
    };
    // Metadata only: no build, network access, or lockfile changes. In particular,
    // do not run cargo doc on a keypress. Bound the subprocess and kill on timeout.
    let mut command = tokio::process::Command::new("cargo");
    command
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .current_dir(manifest.parent().unwrap())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await??;
    anyhow::ensure!(
        output.status.success(),
        "Cannot check workspace documentation: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let manifests = workspace_manifests(&metadata)?;
    for uri in uris {
        let Ok(path) = uri.to_file_path() else {
            return Ok(true);
        };
        // The Rust sysroot is not a Cargo package; it is outside the workspace.
        // Only treat a missing manifest as external for the standard library's
        // known source layout, not arbitrary virtual/generated definitions.
        let Some(manifest) = manifest_for(&path).await else {
            if path.components().any(|part| part.as_os_str() == "rustlib") {
                continue;
            }
            return Ok(true);
        };
        if manifests.contains(&manifest) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn workspace_manifests(metadata: &serde_json::Value) -> anyhow::Result<Vec<PathBuf>> {
    let members = metadata["workspace_members"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing Cargo workspace members"))?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing Cargo packages"))?;
    packages
        .iter()
        .filter(|package| members.contains(&package["id"]))
        .map(|package| {
            let manifest = package["manifest_path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing Cargo manifest path"))?;
            Ok(std::fs::canonicalize(manifest)?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_docs_must_be_existing_files_and_keep_anchors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("struct.Local.html");
        let local = lsp::Url::parse(&format!(
            "{}#method.run",
            lsp::Url::from_file_path(&path).unwrap().as_str()
        ))
        .unwrap();
        let docs = ExternalDocs::WithLocal {
            web: Some(
                lsp::Url::parse("https://docs.rs/example/1/example/struct.Local.html").unwrap(),
            ),
            local: Some(local.clone()),
        };
        assert!(existing_local(&docs).await.is_none());
        std::fs::write(path, "docs").unwrap();
        assert_eq!(existing_local(&docs).await, Some(local));
        let directory_docs = ExternalDocs::WithLocal {
            web: None,
            local: Some(lsp::Url::from_file_path(directory.path()).unwrap()),
        };
        assert!(existing_local(&directory_docs).await.is_none());
    }

    #[test]
    fn accepts_legacy_null_and_paired_responses() {
        for json in [
            "null",
            "\"https://doc.rust-lang.org/std/vec/struct.Vec.html\"",
            r#"{"web":null,"local":null}"#,
        ] {
            serde_json::from_str::<ExternalDocs>(json).unwrap();
        }
    }

    #[test]
    fn only_workspace_members_are_local_even_with_external_packages_in_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("Cargo.toml");
        std::fs::write(&manifest, "").unwrap();
        let metadata = serde_json::json!({"workspace_members": ["local"], "packages": [
            {"id": "local", "manifest_path": manifest},
            {"id": "dependency", "manifest_path": "/does/not/exist/Cargo.toml"}
        ]});
        assert_eq!(
            workspace_manifests(&metadata).unwrap(),
            vec![std::fs::canonicalize(manifest).unwrap()]
        );
        assert!(workspace_manifests(&serde_json::json!({})).is_err());
    }
}
