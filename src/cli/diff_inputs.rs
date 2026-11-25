//! Select inputs before the shared diff pipeline can create an output file.

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use protox::file::{ChainFileResolver, File as ProtoxFile, FileResolver, GoogleFileResolver};

use super::{config::DiffArgs, file_extension, parse_openapi_bytes};
use crate::{
    gitrefs::{MaterializedSpec, Repository},
    models::ApiDocumentation,
};

pub(super) struct Input {
    pub bytes: Vec<u8>,
    path: PathBuf,
    source: String,
    /// The repository session (shared caches and blob reader) and commit of
    /// a Git-ref input.
    snapshot: Option<(Repository, String)>,
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

    fn committed(snapshot: MaterializedSpec, reference: &str, repo: &Repository) -> Self {
        let source = format!(
            "OpenAPI spec at Git ref {reference:?}, path {:?}",
            snapshot.path
        );
        Self {
            snapshot: Some((repo.clone(), snapshot.commit.clone())),
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

    pub fn graphql(
        &self,
        warnings: &mut Vec<String>,
    ) -> Result<crate::contract::graphql::Document> {
        use crate::contract::graphql::Document;
        let label = self.path.display().to_string();
        match &self.sdl_sources {
            Some(sources) => Document::from_sdl_reporting(sources, warnings),
            None => Document::from_bytes_reporting(&self.bytes, &label, warnings),
        }
        .with_context(|| format!("Failed to parse GraphQL schema: {label}"))
    }

    pub fn is_protobuf(&self) -> bool {
        crate::contract::protobuf::is_input(&self.path)
    }

    pub fn protobuf(&self, includes: &[PathBuf]) -> Result<crate::contract::protobuf::Document> {
        use crate::contract::protobuf::{
            Document, compile_with_resolver, parse_proto_source, proto_virtual_name,
        };
        if !self
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
        {
            return Document::decode(&self.bytes);
        }
        let Some((repo, commit)) = &self.snapshot else {
            return crate::contract::protobuf::compile_bytes(&self.path, &self.bytes, includes);
        };
        let mut roots = Vec::with_capacity(includes.len() + 2);
        for include in includes {
            anyhow::ensure!(
                !include.is_absolute()
                    && !include.components().any(|c| matches!(
                        c,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )),
                "Git protobuf import roots must stay inside the committed repository"
            );
            roots.push(normalize_relative_path(include)?);
        }
        // The listing is cached on the shared session, so the import
        // resolver below finds each file's blob without another `ls-tree`.
        let listing = repo
            .listing(commit)
            .context("Failed to enumerate committed protobuf imports")?;
        let mut committed_protos = HashSet::new();
        for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
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
            committed_protos.insert(normalize_relative_path(&path)?);
        }
        let entry_path = normalize_relative_path(&self.path)?;
        roots.push(PathBuf::new());
        if let Some(parent) = entry_path.parent()
            && !parent.as_os_str().is_empty()
            && !roots.iter().any(|r| r == parent)
        {
            roots.push(parent.to_path_buf());
        }
        let entry_virtual = roots
            .iter()
            .filter_map(|root_prefix| {
                if root_prefix.as_os_str().is_empty() {
                    Some(entry_path.clone())
                } else {
                    entry_path
                        .strip_prefix(root_prefix)
                        .ok()
                        .filter(|relative| !relative.as_os_str().is_empty())
                        .map(Path::to_path_buf)
                }
            })
            .next()
            .unwrap_or_else(|| entry_path.clone());
        let entry_name = proto_virtual_name(&entry_virtual)
            .with_context(|| format!("Invalid protobuf path in Git snapshot: {:?}", self.path))?;
        // Match protoc / protox::Compiler::open_file shadowing semantics before
        // serving the entry source ahead of earlier include roots.
        for root_prefix in &roots {
            let candidate = if root_prefix.as_os_str().is_empty() {
                entry_virtual.clone()
            } else {
                root_prefix.join(&entry_virtual)
            };
            if committed_protos.contains(&candidate) {
                if candidate != entry_path {
                    bail!(
                        "Protobuf source {:?} ({entry_name}) is shadowed by {:?} in import root {:?}",
                        entry_path,
                        candidate,
                        if root_prefix.as_os_str().is_empty() {
                            Path::new(".")
                        } else {
                            root_prefix.as_path()
                        }
                    );
                }
                break;
            }
        }
        let entry_source = std::str::from_utf8(&self.bytes)
            .with_context(|| format!("Protobuf source is not valid UTF-8: {:?}", self.path))?
            .to_owned();

        let mut resolver = ChainFileResolver::new();
        resolver.add(GitFileResolver {
            repo: repo.clone(),
            commit: commit.clone(),
            roots,
            committed_protos,
            entry_name: entry_name.clone(),
            entry_source,
            parse_source: parse_proto_source,
        });
        resolver.add(GoogleFileResolver::new());
        compile_with_resolver(resolver, Path::new(&entry_name))
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
            // `.proto` sources enumerate each commit's tree for imports, so
            // let spec lookups share that one listing per commit.
            if [&args.spec, &args.from_spec, &args.to_spec]
                .into_iter()
                .flatten()
                .any(|path| {
                    path.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
                })
            {
                repo.prefer_full_listings();
            }
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
                Input::committed(old, from, &repo),
                Input::committed(new, to, &repo),
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

fn normalize_relative_path(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("Path must be relative and stay inside the repository: {path:?}");
            }
        }
    }
    Ok(normalized)
}

struct GitFileResolver {
    repo: Repository,
    commit: String,
    roots: Vec<PathBuf>,
    committed_protos: HashSet<PathBuf>,
    entry_name: String,
    entry_source: String,
    parse_source: fn(&str, &str) -> Result<ProtoxFile, protox::Error>,
}

impl FileResolver for GitFileResolver {
    fn open_file(&self, name: &str) -> Result<ProtoxFile, protox::Error> {
        if name == self.entry_name {
            return (self.parse_source)(name, &self.entry_source);
        }
        let Ok(rel) = normalize_relative_path(Path::new(name)) else {
            return Err(protox::Error::file_not_found(name));
        };
        if rel.as_os_str().is_empty() {
            return Err(protox::Error::file_not_found(name));
        }
        for root in &self.roots {
            let candidate = if root.as_os_str().is_empty() {
                rel.clone()
            } else {
                root.join(&rel)
            };
            if self.committed_protos.contains(&candidate) {
                let materialized = self
                    .repo
                    .materialize(&self.commit, &candidate)
                    .map_err(|err| protox::Error::new(err.to_string()))?;
                let source = std::str::from_utf8(&materialized.bytes).map_err(|_| {
                    protox::Error::new(format!("Protobuf source is not valid UTF-8: {name:?}"))
                })?;
                return (self.parse_source)(name, source);
            }
        }
        Err(protox::Error::file_not_found(name))
    }
}
