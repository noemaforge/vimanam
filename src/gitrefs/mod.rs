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
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};

/// A discovered, non-bare Git repository, including linked worktrees.
///
/// Clones share one session: tree listings, rename edges and the commits
/// that full object IDs name are cached per object ID (refs such as `HEAD`
/// and abbreviated IDs are re-resolved on every call, so a long-lived
/// repository sees new commits), and committed blobs are read through one
/// long-lived `git cat-file --batch` child instead of a process per blob.
/// The child is closed and reaped when the last clone is dropped.
#[derive(Clone)]
pub struct Repository {
    root: PathBuf,
    session: Arc<Mutex<Session>>,
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repository")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// Per-repository caches. Every key is a full object ID (or a pair of them)
/// and every value is immutable for that ID, so caching cannot change
/// results. Symbolic refs and abbreviated IDs are never cache keys.
#[derive(Default)]
struct Session {
    commits: HashMap<String, String>,
    /// Raw `ls-tree -r --full-tree -z` output per commit.
    listings: HashMap<String, Arc<[u8]>>,
    /// Path-limited regular-file lookups per (commit, path).
    entries: HashMap<(String, PathBuf), Option<Entry>>,
    edges: HashMap<(String, String), Vec<u8>>,
    /// Answer path lookups from one full listing per commit (set when the
    /// caller will need full listings anyway, such as protobuf imports).
    full_listings: bool,
    blobs: BlobState,
}

#[derive(Default)]
enum BlobState {
    #[default]
    Unstarted,
    Running(BlobReader),
    /// The batch reader failed once; use per-blob `git show` from then on so
    /// failures keep their exact messages.
    Disabled,
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
        // One process answers both questions for an ordinary worktree. Any
        // other outcome repeats the original two-step checks so bare, missing
        // and .git-directory cases keep their exact errors.
        let combined = run(
            directory,
            [
                OsStr::new("rev-parse"),
                OsStr::new("--is-bare-repository"),
                OsStr::new("--show-toplevel"),
            ],
        )?;
        if combined.status.success()
            && let Some(root) = combined.stdout.strip_prefix(b"false\n")
            && let Some(root) = root.strip_suffix(b"\n")
            && !root.is_empty()
        {
            return Ok(Self::new(path_bytes(root)?));
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
        Ok(Self::new(root))
    }

    fn new(root: PathBuf) -> Self {
        Self {
            root,
            session: Arc::default(),
        }
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        // Only a full object ID always names the same commit; refs move and
        // an abbreviation can become ambiguous, so those are re-resolved.
        // Entries are only ever stored under IDs of this repository's hash
        // length (see below), so a ref-shaped lookup here never hits.
        let key = is_oid(reference).then(|| reference.to_ascii_lowercase());
        if let Some(commit) = key
            .as_ref()
            .and_then(|k| self.session().commits.get(k).cloned())
        {
            return Ok(commit);
        }
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
        let commit = std::str::from_utf8(&output.stdout)?.trim().to_owned();
        let mut session = self.session();
        // A full tag object ID peels to a fixed commit. Hex of the other
        // hash length (64 in a SHA-1 repository, 40 in SHA-256) is not an
        // object ID there: Git resolves it as a ref name, which can move.
        if let Some(key) = key
            && key.len() == commit.len()
        {
            session.commits.insert(key, commit.clone());
        }
        // A full commit ID resolves to itself, so later lookups by the ID
        // (such as a pinned follow anchor) need no process.
        session.commits.insert(commit.clone(), commit.clone());
        Ok(commit)
    }

    fn at_commit(&self, commit: &str, path: &Path) -> Result<MaterializedSpec> {
        validate_path(path)?;
        let entry = self.lookup(commit, path)?.ok_or_else(|| {
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

    /// The regular file at exactly `path`, from a cached full listing when one
    /// exists (only for already-normal paths, whose bytes the listing holds
    /// verbatim), else from a cached path-limited `ls-tree`.
    fn lookup(&self, commit: &str, path: &Path) -> Result<Option<Entry>> {
        let key = (commit.to_owned(), path.to_path_buf());
        if let Some(entry) = self.session().entries.get(&key) {
            return Ok(entry.clone());
        }
        let normal = path.components().collect::<PathBuf>().as_os_str() == path.as_os_str();
        let (listing, full) = {
            let session = self.session();
            (session.listings.get(commit).cloned(), session.full_listings)
        };
        let entries = match listing {
            Some(listing) if normal => parse_tree(&listing)?,
            None if normal && full => parse_tree(&self.listing(commit)?)?,
            _ => self.tree(commit, Some(path))?,
        };
        let entry = entries.into_iter().find(|e| e.path == path);
        self.session().entries.insert(key, entry.clone());
        Ok(entry)
    }

    /// Read whole-commit listings (once per commit) instead of path-limited
    /// ones, for callers that will enumerate those commits anyway.
    #[cfg(feature = "cli")]
    pub(crate) fn prefer_full_listings(&self) {
        self.session().full_listings = true;
    }

    /// Raw NUL-delimited `ls-tree -r --full-tree -z` records for every entry
    /// (all modes, including symlinks and submodules) at a resolved commit,
    /// listed once per session.
    pub(crate) fn listing(&self, commit: &str) -> Result<Arc<[u8]>> {
        if let Some(listing) = self.session().listings.get(commit) {
            return Ok(listing.clone());
        }
        let bytes: Arc<[u8]> = checked(
            &self.root,
            ["ls-tree", "-r", "--full-tree", "-z", commit, "--"],
            "list committed files",
        )?
        .into();
        self.session()
            .listings
            .insert(commit.to_owned(), bytes.clone());
        Ok(bytes)
    }

    fn tree(&self, commit: &str, path: Option<&Path>) -> Result<Vec<Entry>> {
        let Some(path) = path else {
            return parse_tree(&self.listing(commit)?);
        };
        let mut args = vec![
            OsString::from("ls-tree"),
            "-r".into(),
            "--full-tree".into(),
            "-z".into(),
            commit.into(),
            "--".into(),
        ];
        // The subprocess uses --literal-pathspecs, and returned paths are
        // compared exactly so a directory cannot select its descendants.
        args.push(path.as_os_str().to_owned());
        parse_tree(&checked(&self.root, &args, "list committed files")?)
    }

    /// Read a blob through the session's `git cat-file --batch` child. Any
    /// batch failure (spawn, I/O, missing or non-blob object, malformed
    /// response) disables the batch reader and repeats the read with the
    /// original per-blob `git show`, so errors keep their exact text.
    fn blob(&self, oid: &str) -> Result<Vec<u8>> {
        {
            let mut session = self.session();
            if matches!(session.blobs, BlobState::Unstarted) {
                session.blobs = match BlobReader::spawn(&self.root) {
                    Ok(reader) => BlobState::Running(reader),
                    Err(_) => BlobState::Disabled,
                };
            }
            if let BlobState::Running(reader) = &mut session.blobs {
                if let Some(bytes) = reader.read(oid) {
                    return Ok(bytes);
                }
                // Dropping the reader kills and reaps the child.
                session.blobs = BlobState::Disabled;
            }
        }
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
        // Not cached: a long-lived repository can be deepened or unshallowed.
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
        let verify = changes
            .iter()
            .map(|(child, _)| (edges[child].to_owned(), (*child).to_owned()))
            .collect::<Vec<_>>();
        self.prefetch_edges(&verify);
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

    /// Rename-aware changes between `parent` and `child`, from the session
    /// cache (filled in batches by [`Self::prefetch_edges`]) or one
    /// `diff-tree`, whose errors are reported unchanged.
    fn edge_changes(&self, parent: &str, child: &str) -> Result<Vec<u8>> {
        let key = (parent.to_owned(), child.to_owned());
        if let Some(changes) = self.session().edges.get(&key) {
            return Ok(changes.clone());
        }
        let mut args = vec!["diff-tree", "--no-commit-id"];
        args.extend(EDGE_DIFF_OPTIONS);
        args.extend([parent, child, "--"]);
        let changes = checked(&self.root, args, "inspect renames")?;
        self.session().edges.insert(key, changes.clone());
        Ok(changes)
    }

    /// Diff many `(parent, child)` commit edges in one `git diff-tree
    /// --stdin` process and cache each edge's records exactly as a single
    /// `diff-tree parent child` prints them. This is only an optimization:
    /// on any failure or unexpected output nothing is cached, and
    /// [`Self::edge_changes`] diffs edges one at a time as before.
    fn prefetch_edges(&self, edges: &[(String, String)]) {
        let edges = {
            let session = self.session();
            let mut seen = BTreeSet::new();
            edges
                .iter()
                .filter(|edge| !session.edges.contains_key(*edge) && seen.insert(*edge))
                .collect::<Vec<_>>()
        };
        // A lone edge costs one process either way.
        if edges.len() < 2 || !edges.iter().all(|(p, c)| is_oid(p) && is_oid(c)) {
            return;
        }
        // "<commit> <parent>" diffs the commit against that parent alone.
        let mut input = Vec::new();
        for (parent, child) in &edges {
            input.extend_from_slice(format!("{child} {parent}\n").as_bytes());
        }
        let mut args = vec!["diff-tree", "--stdin", "--always"];
        args.extend(EDGE_DIFF_OPTIONS);
        let Ok(output) = run_with_input(&self.root, args, input) else {
            return;
        };
        if !output.status.success() {
            return;
        }
        let children = edges.iter().map(|(_, c)| c.as_str()).collect::<Vec<_>>();
        let Some(records) = stdin_edge_changes(&output.stdout, &children) else {
            return;
        };
        let mut session = self.session();
        for ((parent, child), changes) in edges.into_iter().zip(records) {
            session
                .edges
                .insert((parent.clone(), child.clone()), changes);
        }
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
        // Every candidate edge in visiting order. Edges are fetched in
        // bounded chunks starting at the first one a route actually needs,
        // so a lineage that ends early wastes at most one chunk.
        let order = nodes
            .iter()
            .flat_map(|node| {
                node[1..]
                    .iter()
                    .filter(|p| allowed.contains(*p))
                    .map(|p| (p.clone(), node[0].clone()))
            })
            .collect::<Vec<_>>();
        let mut position = 0usize;
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
                let index = position;
                position += 1;
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
                if !self
                    .session()
                    .edges
                    .contains_key(&(parent.clone(), child.clone()))
                {
                    let end = order.len().min(index + EDGE_BATCH);
                    self.prefetch_edges(&order[index..end]);
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

/// Options shared by single-edge and `--stdin` rename diffs, so both produce
/// identical records.
const EDGE_DIFF_OPTIONS: [&str; 7] = [
    "--name-status",
    "-r",
    "-z",
    "--no-ext-diff",
    "--no-textconv",
    "-M50%",
    "-l0",
];

/// Edges per `diff-tree --stdin` request.
const EDGE_BATCH: usize = 256;

fn is_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Split `diff-tree --stdin --always -z --name-status` output into one record
/// list per requested child, in request order. Each response is the child ID
/// then `status NUL path NUL [path NUL]` records; paths are consumed by
/// status, so a path that looks like an object ID cannot end a response.
/// Returns `None` for anything unexpected.
fn stdin_edge_changes(bytes: &[u8], children: &[&str]) -> Option<Vec<Vec<u8>>> {
    let bytes = bytes.strip_suffix(b"\0")?;
    let mut fields = bytes.split(|b| *b == 0).peekable();
    let mut records = Vec::with_capacity(children.len());
    for child in children {
        if fields.next()? != child.as_bytes() {
            return None;
        }
        let mut changes = Vec::new();
        while let Some(status) = fields.next_if(|f| edge_status_names(f).is_some()) {
            changes.extend_from_slice(status);
            changes.push(0);
            for _ in 0..edge_status_names(status)? {
                let name = fields.next()?;
                if name.is_empty() {
                    return None;
                }
                changes.extend_from_slice(name);
                changes.push(0);
            }
        }
        records.push(changes);
    }
    fields.next().is_none().then_some(records)
}

/// Paths following a `--name-status` status: two for renames/copies with a
/// score, one for every other status letter.
fn edge_status_names(status: &[u8]) -> Option<usize> {
    match status {
        [b'A' | b'D' | b'M' | b'T' | b'U' | b'X' | b'B'] => Some(1),
        [b'R' | b'C', score @ ..] if !score.is_empty() && score.iter().all(u8::is_ascii_digit) => {
            Some(2)
        }
        _ => None,
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

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    oid: String,
}

/// Regular files (`100644`/`100755` blobs) from NUL-delimited `ls-tree -z`
/// records; symlinks, submodules and trees are skipped.
fn parse_tree(bytes: &[u8]) -> Result<Vec<Entry>> {
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

/// A long-lived `git cat-file --batch` child. It prints raw object bytes:
/// no textconv, filters or external diff apply (`--textconv`/`--filters` are
/// never passed), matching `git show --no-ext-diff --no-textconv <blob>`.
struct BlobReader {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
}

impl BlobReader {
    fn spawn(root: &Path) -> std::io::Result<Self> {
        let mut child = git_command(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Errors are reproduced by the `git show` fallback; an unread
            // stderr pipe could otherwise fill and stall the child.
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new);
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    /// One request/response round trip; `None` on anything unexpected.
    fn read(&mut self, oid: &str) -> Option<Vec<u8>> {
        // Only object IDs from `ls-tree` are sent: a revision expression
        // could otherwise name a different object or contain a newline.
        if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let stdin = self.stdin.as_mut()?;
        stdin.write_all(oid.as_bytes()).ok()?;
        stdin.write_all(b"\n").ok()?;
        stdin.flush().ok()?;
        let stdout = self.stdout.as_mut()?;
        let mut header = Vec::new();
        stdout.read_until(b'\n', &mut header).ok()?;
        let header = std::str::from_utf8(header.strip_suffix(b"\n")?).ok()?;
        let mut fields = header.split(' ');
        let (Some(name), Some("blob"), Some(size), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return None;
        };
        if !name.eq_ignore_ascii_case(oid) {
            return None;
        }
        let size = size.parse::<usize>().ok()?;
        let mut bytes = vec![0; size];
        stdout.read_exact(&mut bytes).ok()?;
        let mut terminator = [0];
        stdout.read_exact(&mut terminator).ok()?;
        (terminator == *b"\n").then_some(bytes)
    }
}

impl Drop for BlobReader {
    fn drop(&mut self) {
        // Closing stdin ends an idle child at EOF; closing stdout makes a
        // child still writing an abandoned response fail with EPIPE. Give it
        // a short bounded grace period to exit on its own, kill it only if
        // it is still running (for example stuck in a lazy fetch), then
        // always reap so no zombie outlives the session.
        drop(self.stdin.take());
        drop(self.stdout.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        loop {
            match self.child.try_wait() {
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    break;
                }
                _ => break,
            }
        }
        let _ = self.child.wait();
    }
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
    git_command(directory).args(args).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!("Git executable not found on PATH")
        } else {
            anyhow!(error).context("Failed to launch Git")
        }
    })
}

/// Run Git with `input` on stdin, written from a separate thread so a full
/// stdout pipe can never deadlock against a full stdin pipe.
fn run_with_input<I, S>(directory: &Path, args: I, input: Vec<u8>) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = git_command(directory)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            // A write error means Git exited early; its status reports it.
            let _ = stdin.write_all(&input);
        }
    });
    let output = child.wait_with_output();
    let _ = writer.join();
    output
}

/// Git with this module's fixed hardening: no pager, replace objects, glob
/// pathspecs, colour, or inherited repository/index/pathspec environment.
fn git_command(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("--no-replace-objects")
        .arg("--literal-pathspecs")
        .args(["-c", "color.ui=false"])
        .arg("-C")
        .arg(directory)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_GLOB_PATHSPECS")
        .env_remove("GIT_NOGLOB_PATHSPECS")
        .env_remove("GIT_ICASE_PATHSPECS");
    command
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
    let yaml = || {
        serde_saphyr::from_slice_with_options::<Header>(bytes, crate::parser::yaml_options()).ok()
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn repository() -> Option<(tempfile::TempDir, Repository)> {
        if !Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "init",
            ],
        );
        let repo = Repository::discover(root).unwrap();
        Some((dir, repo))
    }

    #[test]
    fn batch_reader_rejects_missing_non_blob_and_non_oid_requests() {
        let Some((dir, _)) = repository() else {
            return;
        };
        let root = dir.path();
        let blob = git(root, &["rev-parse", "HEAD:a.txt"]);
        let tree = git(root, &["rev-parse", "HEAD^{tree}"]);
        let mut reader = BlobReader::spawn(root).unwrap();
        assert_eq!(reader.read(&blob).unwrap(), b"alpha\n");
        assert!(reader.read("HEAD:a.txt").is_none());
        assert!(reader.read(&tree).is_none());
        let mut reader = BlobReader::spawn(root).unwrap();
        assert!(reader.read(&"0".repeat(40)).is_none());
    }

    #[test]
    fn batch_failure_falls_back_to_the_original_blob_error() {
        let Some((_dir, repo)) = repository() else {
            return;
        };
        let error = repo.blob(&"0".repeat(40)).unwrap_err().to_string();
        assert!(
            error.starts_with("Git could not read committed blob: "),
            "{error}"
        );
        assert!(matches!(repo.session().blobs, BlobState::Disabled));
        // The per-blob fallback still serves later reads.
        let commit = repo.resolve("HEAD").unwrap();
        assert_eq!(
            repo.at_commit(&commit, Path::new("a.txt")).unwrap().bytes,
            b"alpha\n"
        );
    }

    #[test]
    fn batched_edge_diffs_match_single_edge_diffs() {
        let Some((dir, _)) = repository() else {
            return;
        };
        let root = dir.path();
        let commit = |message: &str| {
            git(root, &["add", "--all"]);
            git(
                root,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    message,
                ],
            );
        };
        let body = "one\ntwo\nthree\nfour\nfive\n";
        // Names that look like statuses or object IDs must stay names.
        let hash_like = "0123456789abcdef0123456789abcdef01234567";
        for name in ["M", "R100", hash_like, "spec.json"] {
            std::fs::write(root.join(name), format!("{name}\n{body}")).unwrap();
        }
        commit("names");
        git(root, &["mv", "spec.json", "renamed.json"]);
        git(root, &["mv", "M", "D"]);
        commit("renames");
        commit("empty");
        git(root, &["checkout", "-qb", "side", "HEAD~2"]);
        std::fs::write(root.join("side.txt"), "side\n").unwrap();
        std::fs::remove_file(root.join(hash_like)).unwrap();
        commit("side");
        git(root, &["checkout", "-q", "-"]);
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "merge",
                "-q",
                "--no-edit",
                "side",
            ],
        );
        let history = git(root, &["rev-list", "--parents", "HEAD"]);
        let edges = history
            .lines()
            .flat_map(|line| {
                let mut ids = line.split(' ');
                let child = ids.next().unwrap().to_owned();
                ids.map(move |parent| (parent.to_owned(), child.clone()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert!(edges.len() >= 5);
        let batched = Repository::discover(root).unwrap();
        batched.prefetch_edges(&edges);
        assert_eq!(batched.session().edges.len(), edges.len());
        let single = Repository::discover(root).unwrap();
        let mut renames = 0;
        for (parent, child) in &edges {
            let expected = single.edge_changes(parent, child).unwrap();
            renames += expected.windows(4).filter(|w| *w == b"\0R10").count();
            assert_eq!(
                batched.session().edges[&(parent.clone(), child.clone())],
                expected,
                "{parent}..{child}"
            );
        }
        assert!(renames >= 2);
    }

    #[test]
    fn unexpected_batched_edge_output_is_rejected() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let ok = format!("{a}\0M\0x\0{b}\0R100\0{b}\0y\0");
        assert_eq!(
            stdin_edge_changes(ok.as_bytes(), &[&a, &b]).unwrap(),
            vec![b"M\0x\0".to_vec(), format!("R100\0{b}\0y\0").into_bytes()]
        );
        for bad in [
            format!("{a}\0M\0x\0"),
            format!("{b}\0{a}\0"),
            format!("{a}\0Q\0x\0{b}\0"),
            format!("{a}\0{b}\0M\0"),
            format!("{a}\0{b}"),
        ] {
            assert!(
                stdin_edge_changes(bad.as_bytes(), &[&a, &b]).is_none(),
                "{bad:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_last_session_reaps_the_batch_child() {
        let Some((_dir, repo)) = repository() else {
            return;
        };
        let commit = repo.resolve("HEAD").unwrap();
        let clone = repo.clone();
        clone.at_commit(&commit, Path::new("a.txt")).unwrap();
        let pid = match &repo.session().blobs {
            BlobState::Running(reader) => reader.child.id(),
            _ => panic!("batch reader did not start"),
        };
        let alive = || {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        drop(clone);
        assert!(alive(), "a remaining clone keeps the session");
        drop(repo);
        assert!(!alive(), "batch child {pid} was not reaped");
    }
}
