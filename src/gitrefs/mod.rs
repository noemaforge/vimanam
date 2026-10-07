//! Discover committed OpenAPI candidates and read spec bytes through Git on PATH.
//!
//! No checkout, temporary file, CLI feature, or console output is required. Paths
//! are repository-relative; refs name commits (tags and revision expressions work).
//! Discovery reads committed blobs, so untracked, ignored and uncommitted files
//! do not affect candidates. Candidate sniffing validates syntax and a small
//! OpenAPI header, not operations or schemas; use [`crate::parse_openapi_bytes`]
//! for complete parsing.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! let repo = vimanam::gitrefs::Repository::discover("api/openapi.yaml")?;
//! let snapshot = repo.materialize("v1.0.0", "api/openapi.yaml")?;
//! let api = vimanam::parse_openapi_bytes(&snapshot.bytes, "yaml", Some(&snapshot.path))?;
//! # Ok(())
//! # }
//! ```

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, de::IgnoredAny};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

/// A discovered, non-bare Git repository, including linked worktrees.
#[derive(Debug, Clone)]
pub struct Repository {
    root: PathBuf,
}

/// Bytes from a regular file in a committed Git tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedSpec {
    /// Resolved commit object ID (rather than a moving branch name).
    pub commit: String,
    /// The actual repository-relative path at that commit.
    pub path: PathBuf,
    /// Exact blob bytes, reusable for hashing and [`crate::parse_openapi_bytes`].
    pub bytes: Vec<u8>,
}

impl Repository {
    /// Find a repository by walking up from a directory or spec path.
    /// Nonexistent spec paths are supported if an ancestor directory exists.
    /// Bare repositories are rejected because this API models a worktree root.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self> {
        let start = start.as_ref();
        let absolute = if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()?.join(start)
        };
        let mut directory = absolute.as_path();
        while !directory.is_dir() {
            directory = directory
                .parent()
                .ok_or_else(|| anyhow!("No existing directory above {}", start.display()))?;
        }
        let bare = run(
            directory,
            [OsStr::new("rev-parse"), OsStr::new("--is-bare-repository")],
        )?;
        if !bare.status.success() {
            bail!("Not a Git repository: {}", start.display());
        }
        if bare.stdout == b"true\n" {
            bail!("Bare Git repository has no worktree: {}", start.display());
        }
        let root = checked(
            directory,
            [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
            "find repository root",
        )?;
        let root = path_bytes(root.strip_suffix(b"\n").unwrap_or(&root))?;
        Ok(Self { root })
    }

    /// Absolute root of the worktree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// List tracked regular JSON/YAML OpenAPI candidates at `reference`, sorted
    /// by repository-relative path. Symlinks and submodules are excluded.
    /// Valid syntax, a `swagger: "2.0"` or `openapi: "3.*"` marker, string
    /// `info.title`/`info.version`, and an object `paths` are required. Detailed
    /// operation/schema validation is intentionally deferred to the parser.
    pub fn candidates(&self, reference: &str) -> Result<Vec<PathBuf>> {
        let commit = self.resolve(reference)?;
        let mut candidates = Vec::new();
        for entry in self.tree(&commit, None)? {
            let extension = entry
                .path
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or_default();
            if !["json", "yaml", "yml"].contains(&extension.to_ascii_lowercase().as_str()) {
                continue;
            }
            let bytes = self.blob(&entry.oid)?;
            if sniff(&bytes, extension) {
                candidates.push(entry.path);
            }
        }
        candidates.sort();
        Ok(candidates)
    }

    /// Read a literal path at a ref without following renames. No files are
    /// created and no worktree content is used. Missing refs, missing paths and
    /// non-regular files produce contextual errors.
    pub fn materialize(&self, reference: &str, path: impl AsRef<Path>) -> Result<MaterializedSpec> {
        let commit = self.resolve(reference)?;
        self.at_commit(&commit, path.as_ref())
    }

    /// Follow the lineage of `path` at `path_ref` to `target_ref`, then read it.
    /// Comparisons use shared history, preferring Git's unique merge base and
    /// falling back to older shared commits when an anchored merge-parent route
    /// does not cross that base.
    /// Additions/deletions stop lineage; an old filename reused for another file
    /// is never a fallback. Multiple merge bases or resulting paths are errors.
    ///
    /// Uses Git's 50% similarity rename heuristic across each parent/child edge,
    /// without copy detection. Heavily rewritten renames, deleted/reintroduced
    /// files and incomplete shallow history may be unresolvable; use literal
    /// [`Self::materialize`] with an explicit path in those cases. Tracking is
    /// file-level, including intermediate non-spec extensions and contents.
    /// Backward ancestor comparisons over a complete linear interval use a
    /// file-focused log, verifying each event with the same parent-edge rules.
    /// Merge, forward, sibling-branch and shallow comparisons retain the full
    /// merge-aware traversal.
    pub fn materialize_follow(
        &self,
        path: impl AsRef<Path>,
        path_ref: &str,
        target_ref: &str,
    ) -> Result<MaterializedSpec> {
        let path = path.as_ref();
        validate_path(path)?;
        let anchor = self.resolve(path_ref)?;
        self.at_commit(&anchor, path)?;
        let target = self.resolve(target_ref)?;
        if anchor == target {
            return self.at_commit(&target, path);
        }
        let bases = run(&self.root, ["merge-base", "--all", &anchor, &target])?;
        if !bases.status.success() {
            bail!(
                "Git could not find common history for {path_ref:?} and {target_ref:?} (check shallow clone history): {}",
                String::from_utf8_lossy(&bases.stderr).trim()
            );
        }
        let bases = std::str::from_utf8(&bases.stdout)?
            .lines()
            .collect::<Vec<_>>();
        if bases.len() != 1 {
            bail!(
                "Ambiguous Git history: expected one merge base for {path_ref:?} and {target_ref:?}, found {}",
                bases.len()
            );
        }
        let primary = bases[0];
        if primary == target
            && let Some(historical_path) = self.follow_linear(&target, &anchor, path)?
            && let Ok(snapshot) = self.at_commit(&target, &historical_path)
        {
            return Ok(snapshot);
        }
        let anchored = self.trace(
            primary,
            &anchor,
            BTreeSet::from([path.to_path_buf()]),
            false,
        )?;
        if anchored.paths.len() > 1 {
            return Err(unique(anchored.paths, path_ref).unwrap_err());
        }
        if let Some(base_path) = anchored.paths.into_iter().next() {
            let target_state = self.trace(primary, &target, BTreeSet::from([base_path]), true)?;
            if target_state.paths.len() > 1 {
                return Err(unique(target_state.paths, target_ref).unwrap_err());
            }
            if let Some(target_path) = target_state.paths.into_iter().next() {
                return self.at_commit(&target, &target_path);
            }
        }

        let mut candidates = Vec::new();
        // A merge base can be a commit on one side of a merge while the
        // explicitly anchored file only has a detectable route through the
        // other parent. In that case an older shared commit can still provide
        // an unambiguous file-level bridge.
        let anchor_history =
            self.git(["rev-list", "--topo-order", &anchor], "read shared history")?;
        let target_history = self.git(["rev-list", &target], "read shared history")?;
        let target_commits = std::str::from_utf8(&target_history)?
            .lines()
            .collect::<BTreeSet<_>>();
        candidates.extend(
            std::str::from_utf8(&anchor_history)?
                .lines()
                .filter(|commit| target_commits.contains(commit) && *commit != primary)
                .map(str::to_owned),
        );

        for base in candidates {
            let anchor_state =
                self.trace(&base, &anchor, BTreeSet::from([path.to_path_buf()]), false)?;
            if anchor_state.paths.len() > 1 {
                return Err(unique(anchor_state.paths, path_ref).unwrap_err());
            }
            let Some(base_path) = anchor_state.paths.into_iter().next() else {
                continue;
            };
            let target_state = self.trace(&base, &target, BTreeSet::from([base_path]), true)?;
            if target_state.paths.len() > 1 {
                return Err(unique(target_state.paths, target_ref).unwrap_err());
            }
            if let Some(target_path) = target_state.paths.into_iter().next() {
                return self.at_commit(&target, &target_path);
            }
        }
        bail!(
            "Spec lineage has no path at {target_ref:?} (file added/deleted, rename not detected, or history incomplete); supply an explicit path"
        )
    }

    fn resolve(&self, reference: &str) -> Result<String> {
        let revision = format!("{reference}^{{commit}}");
        let output = run(
            &self.root,
            [
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--end-of-options"),
                OsStr::new(&revision),
            ],
        )?;
        if !output.status.success() {
            bail!(
                "Git ref does not name an available commit: {reference:?} (check shallow clone history)"
            );
        }
        Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned())
    }

    fn at_commit(&self, commit: &str, path: &Path) -> Result<MaterializedSpec> {
        validate_path(path)?;
        let entry = self
            .tree(commit, Some(path))?
            .into_iter()
            .find(|e| e.path == path)
            .ok_or_else(|| {
                anyhow!(
                    "Git path is missing or is not a regular file at {commit}: {}",
                    path.display()
                )
            })?;
        Ok(MaterializedSpec {
            commit: commit.to_owned(),
            path: entry.path,
            bytes: self.blob(&entry.oid)?,
        })
    }

    fn tree(&self, commit: &str, path: Option<&Path>) -> Result<Vec<Entry>> {
        let mut args = vec![
            OsString::from("ls-tree"),
            "-r".into(),
            "--full-tree".into(),
            "-z".into(),
            commit.into(),
            "--".into(),
        ];
        if let Some(path) = path {
            // The subprocess uses --literal-pathspecs, and returned paths are
            // compared exactly so a directory cannot select its descendants.
            args.push(path.as_os_str().to_owned());
        }
        let bytes = checked(&self.root, &args, "list committed files")?;
        let mut entries = Vec::new();
        for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let (header, name) = record.split_at(
                record
                    .iter()
                    .position(|b| *b == b'\t')
                    .context("Invalid Git tree record")?,
            );
            let fields = std::str::from_utf8(header)?
                .split_whitespace()
                .collect::<Vec<_>>();
            if fields.len() != 3 {
                bail!("Invalid Git tree header");
            }
            if ["100644", "100755"].contains(&fields[0]) && fields[1] == "blob" {
                entries.push(Entry {
                    path: path_bytes(&name[1..])?,
                    oid: fields[2].to_owned(),
                });
            }
        }
        Ok(entries)
    }

    fn blob(&self, oid: &str) -> Result<Vec<u8>> {
        self.git(
            ["show", "--no-ext-diff", "--no-textconv", oid, "--"],
            "read committed blob",
        )
    }

    fn git<const N: usize>(&self, args: [&str; N], operation: &str) -> Result<Vec<u8>> {
        checked(&self.root, args, operation)
    }

    // Only optimize backward comparisons whose complete interval is linear.
    // Path-limited log simplification is not equivalent to our merge semantics.
    fn follow_linear(&self, base: &str, tip: &str, path: &Path) -> Result<Option<PathBuf>> {
        if self.git(
            ["rev-parse", "--is-shallow-repository"],
            "check shallow history",
        )? != b"false\n"
        {
            return Ok(None);
        }
        let range = format!("{base}..{tip}");
        let history = self.git(
            ["rev-list", "--topo-order", "--parents", &range, "--"],
            "read linear history",
        )?;
        let mut edges = BTreeMap::new();
        let mut expected = tip;
        for line in std::str::from_utf8(&history)?.lines() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 2 || fields[0] != expected {
                return Ok(None);
            }
            edges.insert(fields[0], fields[1]);
            expected = fields[1];
        }
        if expected != base {
            return Ok(None);
        }
        let args = [
            OsStr::new("log"),
            OsStr::new("--follow"),
            OsStr::new("--format=%H"),
            OsStr::new("--no-decorate"),
            OsStr::new("--no-show-signature"),
            OsStr::new("--name-status"),
            OsStr::new("-z"),
            OsStr::new("--no-ext-diff"),
            OsStr::new("--no-textconv"),
            OsStr::new("-M50%"),
            OsStr::new("-l0"),
            OsStr::new(&range),
            OsStr::new("--"),
            path.as_os_str(),
        ];
        let history = checked(&self.root, args, "read file history")?;
        let Some(changes) = log_changes(&history, &edges) else {
            return Ok(None);
        };
        let mut historical = path.to_path_buf();
        for (child, changes) in changes {
            let Some(log_path) = map_path(&changes, &historical, false)? else {
                return Ok(None);
            };
            // --follow's path-focused rename search can differ from a full
            // parent-edge diff (especially with copies or competing names).
            // Verify its mapping using exactly the existing engine's rules.
            let changes = self.edge_changes(edges[child], child)?;
            if map_path(&changes, &historical, false)? != Some(log_path.clone()) {
                return Ok(None);
            }
            historical = log_path;
        }
        Ok(Some(historical))
    }

    fn edge_changes(&self, parent: &str, child: &str) -> Result<Vec<u8>> {
        self.git(
            [
                "diff-tree",
                "--no-commit-id",
                "--name-status",
                "-r",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "-M50%",
                "-l0",
                parent,
                child,
                "--",
            ],
            "inspect renames",
        )
    }

    fn trace(
        &self,
        base: &str,
        tip: &str,
        initial: BTreeSet<PathBuf>,
        forward: bool,
    ) -> Result<TraceState> {
        if base == tip {
            return Ok(TraceState {
                paths: initial,
                broken: false,
            });
        }
        let range = format!("{base}..{tip}");
        let bytes = self.git(
            [
                "rev-list",
                "--reverse",
                "--topo-order",
                "--parents",
                &range,
                "--",
            ],
            "read rename history",
        )?;
        let mut nodes = std::str::from_utf8(&bytes)?
            .lines()
            .map(|line| {
                line.split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let allowed = nodes
            .iter()
            .map(|n| n[0].clone())
            .chain([base.to_owned()])
            .collect::<BTreeSet<_>>();
        if !forward {
            nodes.reverse();
        }
        let mut states = BTreeMap::from([(
            if forward { base } else { tip }.to_owned(),
            TraceState {
                paths: initial,
                broken: false,
            },
        )]);
        for node in nodes {
            let child = &node[0];
            let merge = node.len() > 2;
            let mut edge_breaks = Vec::new();
            let mut active_routes = 0usize;
            for parent in node[1..].iter().filter(|p| allowed.contains(*p)) {
                let (from, to) = if forward {
                    (parent, child)
                } else {
                    (child, parent)
                };
                let Some(state) = states.get(from).cloned() else {
                    continue;
                };
                if state.broken {
                    states.entry(to.clone()).or_default().broken = true;
                }
                if state.paths.is_empty() {
                    continue;
                }
                let changes = self.edge_changes(parent, child)?;
                for path in state.paths {
                    if let Some(mapped) = map_path(&changes, &path, forward)? {
                        active_routes += 1;
                        states.entry(to.clone()).or_default().insert(mapped);
                    } else {
                        edge_breaks.push(to.clone());
                    }
                }
            }
            if let Some(to) = edge_breaks.into_iter().next() {
                // An added path on one merge parent can be the same anchored
                // lineage that arrived under a different name on another
                // parent. The actual historical break is retained in the
                // parent's state; this merge-edge addition alone is not proof
                // of a second file lifetime.
                if !merge || active_routes == 0 {
                    states.entry(to).or_default().broken = true;
                }
            }
        }
        let result = states
            .remove(if forward { tip } else { base })
            .unwrap_or_default();
        if result.broken && !result.paths.is_empty() {
            bail!(
                "Ambiguous spec lineage at merge history near {tip}: a tracked route is broken while another survives; supply an explicit path"
            );
        }
        Ok(result)
    }
}

#[derive(Clone, Default)]
struct TraceState {
    paths: BTreeSet<PathBuf>,
    broken: bool,
}

impl TraceState {
    fn insert(&mut self, path: PathBuf) {
        self.paths.insert(path);
    }
}

struct Entry {
    path: PathBuf,
    oid: String,
}

fn unique(paths: BTreeSet<PathBuf>, reference: &str) -> Result<PathBuf> {
    match paths.len() {
        0 => bail!(
            "Spec lineage has no path at {reference:?} (file added/deleted, rename not detected, or history incomplete); supply an explicit path"
        ),
        1 => Ok(paths.into_iter().next().unwrap()),
        _ => bail!("Ambiguous spec paths at {reference:?}: {paths:?}; supply an explicit path"),
    }
}

// Parse only machine records: %H, NUL, LF + status, NUL, literal path(s), NUL.
// Paths are consumed according to status, so hash-like names/newlines cannot
// become commit delimiters. Unexpected output conservatively uses the fallback.
fn log_changes<'a>(
    bytes: &'a [u8],
    edges: &BTreeMap<&str, &str>,
) -> Option<Vec<(&'a str, Vec<u8>)>> {
    let mut fields = bytes.split(|b| *b == 0).peekable();
    let mut records = Vec::new();
    let mut seen = BTreeSet::new();
    while let Some(commit) = fields.next() {
        if commit.is_empty() && fields.peek().is_none() {
            break;
        }
        let commit = std::str::from_utf8(commit).ok()?;
        if !edges.contains_key(commit) || !seen.insert(commit) {
            return None;
        }
        let mut changes = Vec::new();
        let status = fields.next()?.strip_prefix(b"\n")?;
        let mut status = status;
        loop {
            let names = match status {
                b"A" | b"D" | b"M" | b"T" => 1,
                s if s.starts_with(b"R")
                    && s.len() > 1
                    && s[1..].iter().all(u8::is_ascii_digit) =>
                {
                    2
                }
                _ => return None,
            };
            changes.extend_from_slice(status);
            changes.push(0);
            for _ in 0..names {
                let name = fields.next()?;
                if name.is_empty() {
                    return None;
                }
                changes.extend_from_slice(name);
                changes.push(0);
            }
            let Some(next) = fields.peek() else {
                break;
            };
            if next.is_empty() || std::str::from_utf8(next).is_ok_and(|s| edges.contains_key(s)) {
                break;
            }
            status = fields.next()?;
        }
        records.push((commit, changes));
    }
    Some(records)
}

fn map_path(bytes: &[u8], path: &Path, forward: bool) -> Result<Option<PathBuf>> {
    let mut fields = bytes.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(status) = fields.next() {
        let name = path_bytes(fields.next().context("Invalid Git change record")?)?;
        if status.starts_with(b"R") {
            let destination = path_bytes(fields.next().context("Invalid Git rename record")?)?;
            if forward && name == path {
                return Ok(Some(destination));
            }
            if !forward && destination == path {
                return Ok(Some(name));
            }
        } else if (forward && status == b"D" || !forward && status == b"A") && name == path {
            return Ok(None);
        }
    }
    Ok(Some(path.to_path_buf()))
}

fn validate_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!(
            "Git spec path must be a normalized repository-relative path: {}",
            path.display()
        );
    }
    Ok(())
}

fn run<I, S>(directory: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("--no-pager")
        .arg("--no-replace-objects")
        .arg("--literal-pathspecs")
        .args(["-c", "color.ui=false"])
        .arg("-C")
        .arg(directory)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_GLOB_PATHSPECS")
        .env_remove("GIT_NOGLOB_PATHSPECS")
        .env_remove("GIT_ICASE_PATHSPECS")
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow!("Git executable not found on PATH")
            } else {
                anyhow!(error).context("Failed to launch Git")
            }
        })
}

fn checked<I, S>(directory: &Path, args: I, operation: &str) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run(directory, args)?;
    if !output.status.success() {
        bail!(
            "Git could not {operation}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn path_bytes(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(
            std::str::from_utf8(bytes).context("Git path is not UTF-8")?,
        ))
    }
}

#[derive(Deserialize)]
struct Header {
    openapi: Option<String>,
    swagger: Option<String>,
    info: Info,
    paths: BTreeMap<String, IgnoredAny>,
}
#[derive(Deserialize)]
struct Info {
    title: String,
    version: String,
}

fn sniff(bytes: &[u8], extension: &str) -> bool {
    let json = || serde_json::from_slice::<Header>(bytes).ok();
    let yaml = || serde_norway::from_slice::<Header>(bytes).ok();
    let header = if extension.eq_ignore_ascii_case("json") {
        json().or_else(yaml)
    } else {
        yaml().or_else(json)
    };
    header.is_some_and(|h| {
        // Read the header fields while leaving the large operation/schema
        // payloads as IgnoredAny. Empty title/version/paths remain candidates.
        let _ = (&h.info.title, &h.info.version, &h.paths);
        h.swagger.as_deref() == Some("2.0") || h.openapi.is_some_and(|v| v.starts_with("3."))
    })
}
