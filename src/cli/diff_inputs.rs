//! Select inputs before the shared diff pipeline can create an output file.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::{config::DiffArgs, file_extension, parse_openapi_bytes};
use crate::{
    gitrefs::{MaterializedSpec, Repository},
    models::ApiDocumentation,
};

pub(super) struct Input {
    pub bytes: Vec<u8>,
    path: PathBuf,
    source: String,
    snapshot: Option<(PathBuf, String)>,
    /// Merged GraphQL SDL files when the input is a directory.
    sdl_sources: Option<Vec<(String, String)>>,
}

impl Input {
    fn file(path: &Path) -> Result<Self> {
        if path.is_dir() {
            // Directory inputs are merged GraphQL SDL. The hashed bytes are
            // each relative path and file content, NUL-separated, in merge order.
            let sources = crate::contract::graphql::read_sdl_dir(path)
                .with_context(|| format!("Failed to read GraphQL schema directory: {path:?}"))?;
            let mut bytes = Vec::new();
            for (file, text) in &sources {
                let relative = Path::new(file)
                    .strip_prefix(path)
                    .unwrap_or(Path::new(file));
                bytes.extend_from_slice(relative.to_string_lossy().as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(text.as_bytes());
                bytes.push(0);
            }
            return Ok(Self {
                bytes,
                path: path.to_path_buf(),
                source: format!("GraphQL schema directory: {path:?}"),
                snapshot: None,
                sdl_sources: Some(sources),
            });
        }
        let source = if crate::contract::graphql::is_sdl_path(path) {
            format!("GraphQL schema: {path:?}")
        } else {
            format!("OpenAPI file: {path:?}")
        };
        let bytes = fs::read(path).with_context(|| format!("Failed to parse {source}"))?;
        Ok(Self {
            bytes,
            path: path.to_path_buf(),
            source,
            snapshot: None,
            sdl_sources: None,
        })
    }

    fn committed(snapshot: MaterializedSpec, reference: &str, root: &Path) -> Self {
        let source = format!(
            "OpenAPI spec at Git ref {reference:?}, path {:?}",
            snapshot.path
        );
        Self {
            snapshot: Some((root.to_path_buf(), snapshot.commit.clone())),
            bytes: snapshot.bytes,
            path: snapshot.path,
            source,
            sdl_sources: None,
        }
    }

    /// GraphQL SDL file or directory, or introspection JSON (sniffed from
    /// the exact bytes, so Git-ref inputs are recognised too).
    pub fn is_graphql(&self) -> bool {
        self.sdl_sources.is_some()
            || crate::contract::graphql::is_sdl_path(&self.path)
            || crate::contract::graphql::is_introspection_bytes(&self.bytes)
    }

    pub fn graphql(&self) -> Result<crate::contract::graphql::Document> {
        use crate::contract::graphql::Document;
        let label = self.path.display().to_string();
        match &self.sdl_sources {
            Some(sources) => Document::from_sdl(sources),
            None => Document::from_bytes(&self.bytes, &label),
        }
        .with_context(|| format!("Failed to parse GraphQL schema: {label}"))
    }

    pub fn is_protobuf(&self) -> bool {
        crate::contract::protobuf::is_input(&self.path)
    }

    pub fn protobuf(&self, includes: &[PathBuf]) -> Result<crate::contract::protobuf::Document> {
        use crate::contract::protobuf::Document;
        if !self
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
        {
            return Document::decode(&self.bytes);
        }
        let Some((root, commit)) = &self.snapshot else {
            return crate::contract::protobuf::compile_bytes(&self.path, &self.bytes, includes);
        };
        for include in includes {
            anyhow::ensure!(
                !include.is_absolute()
                    && !include.components().any(|c| matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )),
                "Git protobuf import roots must stay inside the committed repository"
            );
        }
        let temp = tempfile::tempdir()?;
        let repo = Repository::discover(root)?;
        let listing = std::process::Command::new("git")
            .current_dir(root)
            .args(["ls-tree", "-rz", "--full-tree", commit])
            .output()?;
        anyhow::ensure!(
            listing.status.success(),
            "Failed to enumerate committed protobuf imports"
        );
        for record in listing.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .context("Malformed Git tree entry")?;
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(&record[tab + 1..]))
            };
            #[cfg(not(unix))]
            let path = PathBuf::from(std::str::from_utf8(&record[tab + 1..])?);
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
            {
                continue;
            }
            anyhow::ensure!(
                record.starts_with(b"100644 ") || record.starts_with(b"100755 "),
                "Historical protobuf imports must be regular committed files: {:?}",
                path
            );
            let target = temp.path().join(&path);
            std::fs::create_dir_all(target.parent().unwrap())?;
            std::fs::write(target, repo.materialize(commit, &path)?.bytes)?;
        }
        let mut roots = includes
            .iter()
            .map(|p| temp.path().join(p))
            .collect::<Vec<_>>();
        roots.push(temp.path().to_path_buf());
        Document::read(&temp.path().join(&self.path), &roots)
    }

    pub fn parse(&self) -> Result<ApiDocumentation> {
        parse_openapi_bytes(&self.bytes, &file_extension(&self.path), Some(&self.path))
            .with_context(|| format!("Failed to parse {}", self.source))
    }
}

pub(super) fn read(args: &DiffArgs) -> Result<(Input, Input)> {
    match (&args.old, &args.new, &args.from_ref, &args.to_ref) {
        (Some(old), Some(new), None, None) => Ok((Input::file(old)?, Input::file(new)?)),
        (None, None, Some(from), Some(to)) => {
            let repo = Repository::discover(std::env::current_dir()?)?;
            let (old, new) = if let (Some(old_path), Some(new_path)) =
                (&args.from_spec, &args.to_spec)
            {
                (
                    repo.materialize(from, old_path)?,
                    repo.materialize(to, new_path)?,
                )
            } else {
                let path = match &args.spec {
                    Some(path) => path.clone(),
                    None => unique_candidate(&repo, to)?,
                };
                let new = repo.materialize(to, &path)?;
                // Pin the anchor to the commit whose bytes we just read.
                let old = repo.materialize_follow(&path, &new.commit, from)
                    .with_context(|| format!(
                        "Failed to follow spec {:?} from Git ref {to:?} to {from:?}; use --from-spec PATH --to-spec PATH to select literal paths at each ref",
                        path
                    ))?;
                (old, new)
            };
            Ok((
                Input::committed(old, from, repo.root()),
                Input::committed(new, to, repo.root()),
            ))
        }
        _ => bail!("Provide OLD NEW or both --from-ref and --to-ref"),
    }
}

fn unique_candidate(repo: &Repository, reference: &str) -> Result<PathBuf> {
    let candidates = repo.candidates(reference)?;
    match candidates.as_slice() {
        [path] => Ok(path.clone()),
        [] => bail!(
            "No tracked OpenAPI candidates at Git ref {reference:?}; use --spec PATH (at --to-ref), or --from-spec PATH --to-spec PATH"
        ),
        _ => bail!(
            "Ambiguous OpenAPI candidates at Git ref {reference:?}:\n{}\nSelect --spec PATH (at --to-ref), or --from-spec PATH --to-spec PATH",
            candidates
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}
