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
}

impl Input {
    fn file(path: &Path) -> Result<Self> {
        let source = format!("OpenAPI file: {path:?}");
        let bytes = fs::read(path).with_context(|| format!("Failed to parse {source}"))?;
        Ok(Self {
            bytes,
            path: path.to_path_buf(),
            source,
        })
    }

    fn committed(snapshot: MaterializedSpec, reference: &str) -> Self {
        let source = format!(
            "OpenAPI spec at Git ref {reference:?}, path {:?}",
            snapshot.path
        );
        Self {
            bytes: snapshot.bytes,
            path: snapshot.path,
            source,
        }
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
            Ok((Input::committed(old, from), Input::committed(new, to)))
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
