//! The per-edition dependency-closure contract.
//!
//! Every edition is a crate in this workspace, and the cheap editions must not
//! grow a geometry kernel by accident: `ferrocad_toy` is documented as
//! kernel-free, `ferrocad_py` is the App-only image, and the GUI/renderer layer
//! sits below Part. Nothing enforced that before, so a single ordinary
//! `[dependencies]` line could pull OCCT into the toy build unnoticed.
//!
//! `cargo xtask editions --check` reads `cargo metadata --format-version 1
//! --no-deps` and walks the *normal* dependency edges from each edition. It then
//! (a) applies the rules below and (b) compares the closure with the pinned
//! `contracts/editions.json`, so an added dependency is a reviewed diff rather
//! than silent drift. `--write` regenerates the contract.
//!
//! # The walk
//!
//! * Only `kind: normal` edges count. Dev- and build-dependencies are dropped —
//!   notably `ferrocad_part`'s dev-dependency on `ferrocad_occt`.
//! * A dependency is *reachable* only if it is unconditional or an optional
//!   dependency that a default-reachable feature turns on. Features are resolved
//!   with a monotone fixpoint over `dep:x`, `x/feat` and `x?/feat` entries; the
//!   root edition starts with its `default` feature enabled (what
//!   `cargo build -p <edition>` does). Unioning features from several dependents
//!   is the same unification cargo performs, so the result is the set of crates
//!   the edition actually links.
//! * The closure records workspace crates by name plus any kernel crate: exactly
//!   `opencascade-sys` or any name starting with `opencascade`, wherever it is
//!   declared. Other third-party crates are not pinned (they are not what leaks
//!   the kernel, and their versions belong in `Cargo.lock`). The edition itself
//!   is part of its closure, the way `cargo tree` prints its root.
//! * Target-specific dependencies (`[target.'cfg(windows)'.dependencies]`) are
//!   not filtered: the contract is one platform-independent set, so a kernel leak
//!   on any platform fails the same check on Linux.
//! * `--no-deps` is deliberate. It makes the command a manifest read: no
//!   registry access, no network, no `.occt/prefix`, no compilation, tens of
//!   milliseconds. The simplification it buys is that a kernel crate reached
//!   through a *third-party* crate is invisible; every path in this workspace
//!   goes through `ferrocad_occt`, which is a workspace crate, so a leak is
//!   still caught at that hop. If a non-workspace crate ever depends on
//!   OCCT directly, this is the file to revisit.
//!
//! # The one carve-out
//!
//! `ferrocad_geom` is the documented geometry seam and re-exports
//! `ferrocad_curves` (`docs/developer/architecture/part-draft-layering.md`), so
//! every light edition reaches the pure geometry through it. `ferrocad_curves`
//! and `ferrocad_mesh` are therefore forbidden *except* along that seam: a direct
//! edge, or a route through a heavier crate, still fails the rule with the
//! offending path.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::json::{self, Value};

/// Where `--write` pins the closures, relative to the workspace root.
const CONTRACT: &str = "contracts/editions.json";

/// Any crate whose name starts with this is a geometry kernel, even if it is not
/// a workspace member (`opencascade-sys` is a patched third-party crate).
const KERNEL_PREFIX: &str = "opencascade";

/// The crates a lightweight edition must never reach: the kernel wrapper, the
/// Part workbench and its bindings, the mesh geometry layer, and OCCT itself.
///
/// `ferrocad_curves` and `ferrocad_mesh` are pure Rust, but the layering keeps
/// even the pure geometry out of the shell, so depending on them from a light
/// edition is a design decision. They are listed here and reachable *only*
/// through [`GEOMETRY_SEAM`] — see [`SEAM_ONLY`].
const KERNEL_FREE: &[&str] = &[
    "ferrocad_occt",
    "ferrocad_part",
    "ferrocad_part_py",
    "ferrocad_part_gui",
    "ferrocad_curves",
    "ferrocad_mesh",
    "ferrocad_mesh_py",
    "opencascade-sys",
];

/// The documented geometry seam: `ferrocad_geom` is the layer that re-exports
/// `ferrocad_curves` (`docs/developer/architecture/part-draft-layering.md`: the
/// layering is `ferrocad_curves -> ferrocad_types`,
/// `ferrocad_geom -> ferrocad_types, ferrocad_curves`). Every light edition already
/// depends on `ferrocad_geom` for `Shape`, so the pure geometry leaves arrive
/// through it by design.
const GEOMETRY_SEAM: &str = "ferrocad_geom";

/// Pure geometry crates whose only legitimate route into a light edition is
/// [`GEOMETRY_SEAM`]. Reaching either by any other route — a direct dependency,
/// or through a heavier crate — is the leak the rules are about.
const SEAM_ONLY: &[&str] = &["ferrocad_curves", "ferrocad_mesh"];

/// One edition's dependency rule.
#[derive(Debug, Clone, Copy)]
struct Rule {
    /// The edition crate whose closure is walked.
    package: &'static str,
    /// What this edition is, for the failure message.
    reason: &'static str,
    /// Crates that must not appear in its closure.
    forbidden: &'static [&'static str],
    /// Crates that must appear in its closure.
    required: &'static [&'static str],
}

/// Does this rule forbid `krate`? An `opencascade-sys` entry covers every
/// `opencascade*` crate, so a renamed kernel split cannot slip past.
fn forbids(rule: &Rule, krate: &str) -> bool {
    rule.forbidden
        .iter()
        .any(|name| *name == krate || (*name == "opencascade-sys" && is_kernel(krate)))
}

/// The edition contract. The order is the order `--check` prints.
const RULES: &[Rule] = &[
    Rule {
        package: "ferrocad",
        reason: "the base app ships no kernel",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_toy",
        reason: "the toy edition is deliberately kernel-free",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_gpui",
        reason: "the shell sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_stage",
        reason: "the interaction seam sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_viewport",
        reason: "the viewport sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_widgets",
        reason: "the widget kit sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_inventor",
        reason: "the scene core sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_wgpu",
        reason: "the renderer sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_ui",
        reason: "the UI vocabulary sits below Part",
        forbidden: KERNEL_FREE,
        required: &[],
    },
    Rule {
        package: "ferrocad_py",
        reason: "the standalone Python image is App-only",
        forbidden: &["ferrocad_part_py", "ferrocad_mesh_py", "ferrocad_occt"],
        required: &[],
    },
    Rule {
        package: "ferrocad_parametric",
        reason: "the parametric edition links the Part workbench",
        forbidden: &[],
        required: &["ferrocad_part_py", "ferrocad_occt"],
    },
    Rule {
        package: "ferrocad_combined_py",
        reason: "the combined image links Part and Mesh",
        forbidden: &[],
        required: &["ferrocad_part_py", "ferrocad_mesh_py"],
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepKind {
    Normal,
    Dev,
    Build,
}

/// One edge out of a package, in the shape `cargo metadata` reports it.
#[derive(Debug, Clone)]
struct Edge {
    /// The real package name (not the rename).
    package: String,
    /// The name features use for this edge: `rename` if present, else `package`.
    alias: String,
    kind: DepKind,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Package {
    /// `[features]`, as declared (`default` included when the crate has one).
    features: BTreeMap<String, Vec<String>>,
    edges: Vec<Edge>,
}

/// The workspace's normal-dependency graph, as read from `cargo metadata`.
#[derive(Debug, Clone, Default)]
struct Graph {
    packages: BTreeMap<String, Package>,
}

/// A package's activated features and the optional edges those features enable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Activation {
    /// Activated feature names of this package.
    features: BTreeSet<String>,
    /// Aliases of optional dependencies switched on by `dep:x` or `x/feat`.
    optional: BTreeSet<String>,
    /// Features requested on a dependency alias (`x/feat`); applied only if that
    /// edge is enabled, which is what makes `x?/feat` conditional.
    requested: BTreeMap<String, BTreeSet<String>>,
}

/// The reachable set for one edition, plus the enabled edges that produced it.
#[derive(Debug, Clone)]
struct Closure {
    /// Workspace crates plus kernel crates, including the edition itself.
    crates: BTreeSet<String>,
    /// `(from, to)` over enabled normal edges, sorted and deduplicated.
    edges: Vec<(String, String)>,
}

impl Graph {
    fn from_metadata(metadata: &Value) -> Result<Self, String> {
        let packages = metadata
            .field("packages")
            .and_then(Value::as_array)
            .ok_or("cargo metadata: no `packages` array")?;
        let mut graph = Graph::default();
        for package in packages {
            let name = package
                .field("name")
                .and_then(Value::as_str)
                .ok_or("cargo metadata: package without a name")?
                .to_string();
            let mut features = BTreeMap::new();
            if let Some(map) = package.field("features").and_then(Value::as_object) {
                for (feature, items) in map {
                    features.insert(feature.clone(), strings(items)?);
                }
            }
            let mut edges = Vec::new();
            for dependency in package
                .field("dependencies")
                .and_then(Value::as_array)
                .unwrap_or(&[])
            {
                let package = dependency
                    .field("name")
                    .and_then(Value::as_str)
                    .ok_or("cargo metadata: dependency without a name")?
                    .to_string();
                let rename = dependency.field("rename").and_then(Value::as_str);
                let alias = rename.unwrap_or(&package).to_string();
                let kind = match dependency.field("kind").and_then(Value::as_str) {
                    None => DepKind::Normal,
                    Some("dev") => DepKind::Dev,
                    Some("build") => DepKind::Build,
                    Some(other) => {
                        return Err(format!(
                            "cargo metadata: package `{package}` has unsupported dependency kind `{other}`"
                        ));
                    }
                };
                edges.push(Edge {
                    package,
                    alias,
                    kind,
                    optional: dependency
                        .field("optional")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    default_features: dependency
                        .field("uses_default_features")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    features: dependency
                        .field("features")
                        .map(strings)
                        .transpose()?
                        .unwrap_or_default(),
                });
            }
            graph.packages.insert(name, Package { features, edges });
        }
        Ok(graph)
    }

    /// Resolve feature activation for everything reachable from `root`.
    fn activated(&self, root: &str) -> Result<BTreeMap<String, Activation>, String> {
        let Some(package) = self.packages.get(root) else {
            return Err(format!(
                "cargo metadata has no package `{root}`; was it renamed or removed from the workspace?"
            ));
        };
        let mut states = BTreeMap::new();
        let mut initial = Activation::default();
        // `cargo build -p <edition>` builds with default features on.
        if package.features.contains_key("default") {
            initial.features.insert("default".to_string());
        }
        states.insert(root.to_string(), initial);

        // Features and reachability only grow, so this terminates; each pass
        // propagates one more step of feature unification.
        loop {
            let mut changed = false;
            let names: Vec<String> = states.keys().cloned().collect();
            for name in names {
                let before = states.get(&name).cloned().unwrap_or_default();
                let mut state = before.clone();
                let package = &self.packages[&name];
                expand_features(package, &mut state);

                for edge in &package.edges {
                    if edge.kind != DepKind::Normal {
                        continue;
                    }
                    if edge.optional && !state.optional.contains(&edge.alias) {
                        continue;
                    }
                    let Some(dependency) = self.packages.get(&edge.package) else {
                        // A third-party crate: only kernel-prefixed names enter
                        // the closure, and they are leaves in this graph.
                        continue;
                    };
                    let newly_reached = !states.contains_key(&edge.package);
                    let child = states.entry(edge.package.clone()).or_default();
                    changed |= newly_reached;
                    if edge.default_features && dependency.features.contains_key("default") {
                        changed |= child.features.insert("default".to_string());
                    }
                    for feature in &edge.features {
                        changed |= child.features.insert(feature.clone());
                    }
                    if let Some(requested) = state.requested.get(&edge.alias) {
                        for feature in requested {
                            changed |= child.features.insert(feature.clone());
                        }
                    }
                }

                if state != before {
                    changed = true;
                    states.insert(name, state);
                }
            }
            if !changed {
                break;
            }
        }
        Ok(states)
    }

    /// The reachable closure of `root`: workspace crates and kernel crates.
    fn closure(&self, root: &str) -> Result<Closure, String> {
        let states = self.activated(root)?;
        let mut crates = BTreeSet::new();
        let mut edges = Vec::new();
        for (name, state) in &states {
            crates.insert(name.clone());
            for edge in &self.packages[name].edges {
                if edge.kind != DepKind::Normal {
                    continue;
                }
                if edge.optional && !state.optional.contains(&edge.alias) {
                    continue;
                }
                if edge.package == *name {
                    continue;
                }
                if self.packages.contains_key(&edge.package) || is_kernel(&edge.package) {
                    crates.insert(edge.package.clone());
                    edges.push((name.clone(), edge.package.clone()));
                }
            }
        }
        edges.sort();
        edges.dedup();
        Ok(Closure { crates, edges })
    }
}

/// Expand a package's activated features until closed over `dep:` / `x/f` / `x?/f`.
fn expand_features(package: &Package, state: &mut Activation) {
    let mut queue: Vec<String> = state.features.iter().cloned().collect();
    while let Some(feature) = queue.pop() {
        let Some(items) = package.features.get(&feature) else {
            continue;
        };
        for item in items {
            if let Some(dependency) = item.strip_prefix("dep:") {
                state.optional.insert(dependency.to_string());
            } else if let Some((alias, wanted)) = item.split_once('/') {
                // `x/feat` switches an optional dependency on; `x?/feat` only
                // requests the feature if something else enabled it. Requesting
                // is recorded either way and applied at the edge, where the
                // enabled/disabled decision is known.
                let alias = match alias.strip_suffix('?') {
                    Some(alias) => alias,
                    None => {
                        state.optional.insert(alias.to_string());
                        alias
                    }
                };
                state
                    .requested
                    .entry(alias.to_string())
                    .or_default()
                    .insert(wanted.to_string());
            } else if state.features.insert(item.clone()) {
                queue.push(item.clone());
            } else if is_optional_edge(package, item) {
                // Implicit optional-dependency feature: `foo = { optional = true }`
                // may be enabled by the bare name `foo`.
                state.optional.insert(item.clone());
            }
        }
    }
}

fn is_optional_edge(package: &Package, alias: &str) -> bool {
    package
        .edges
        .iter()
        .any(|edge| edge.kind == DepKind::Normal && edge.optional && edge.alias == alias)
}

fn is_kernel(name: &str) -> bool {
    name.starts_with(KERNEL_PREFIX)
}

fn strings(value: &Value) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or("cargo metadata: expected an array of strings")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| "cargo metadata: expected a string".to_string())
        })
        .collect()
}

/// A shortest path `from -> … -> to` over the enabled edges that never enters
/// `avoid`, if one exists.
fn path_avoiding(
    closure: &Closure,
    from: &str,
    to: &str,
    avoid: Option<&str>,
) -> Option<Vec<String>> {
    if from == to {
        return Some(vec![from.to_string()]);
    }
    let mut parents: BTreeMap<&str, &str> = BTreeMap::new();
    let mut queue = VecDeque::from([from]);
    let mut seen = BTreeSet::from([from]);
    while let Some(node) = queue.pop_front() {
        for (source, target) in &closure.edges {
            if source != node || Some(target.as_str()) == avoid || !seen.insert(target.as_str()) {
                continue;
            }
            parents.insert(target.as_str(), source.as_str());
            if target == to {
                let mut walk = vec![to.to_string()];
                let mut current = to;
                while let Some(parent) = parents.get(current) {
                    walk.push(parent.to_string());
                    current = parent;
                }
                walk.reverse();
                return Some(walk);
            }
            queue.push_back(target.as_str());
        }
    }
    None
}

/// The route that makes `krate` a violation, if any.
///
/// A [`SEAM_ONLY`] crate is only a violation when it is reachable without going
/// through [`GEOMETRY_SEAM`]; the seam route is the documented arrangement.
fn offending_route(closure: &Closure, from: &str, krate: &str) -> Option<Vec<String>> {
    let avoid = if SEAM_ONLY.contains(&krate) && krate != GEOMETRY_SEAM {
        Some(GEOMETRY_SEAM)
    } else {
        None
    };
    path_avoiding(closure, from, krate, avoid)
}

/// Apply the rules, returning one precise message per violation.
fn check_rules(
    closures: &BTreeMap<String, Closure>,
    rules: &[Rule],
) -> Result<Vec<String>, String> {
    let mut violations = Vec::new();
    for rule in rules {
        let closure = closures
            .get(rule.package)
            .ok_or_else(|| format!("no closure computed for `{}`", rule.package))?;
        for krate in closure.crates.iter() {
            if !forbids(rule, krate) {
                continue;
            }
            let route = offending_route(closure, rule.package, krate);
            if route.is_none() && SEAM_ONLY.contains(&krate.as_str()) {
                // Reachable only through the geometry seam, which is allowed.
                continue;
            }
            let mut message = format!(
                "`{}` must not reach `{krate}` (rule: {})",
                rule.package, rule.reason
            );
            if let Some(route) = route {
                message.push_str(&format!("\n  path: {}", route.join(" -> ")));
            }
            violations.push(message);
        }
        for krate in rule.required {
            if !closure.crates.contains(*krate) {
                violations.push(format!(
                    "`{}` must reach `{krate}` (rule: {}), but it is not in the {}-crate closure",
                    rule.package,
                    rule.reason,
                    closure.crates.len()
                ));
            }
        }
    }
    Ok(violations)
}

fn compute(graph: &Graph, rules: &[Rule]) -> Result<BTreeMap<String, Closure>, String> {
    let mut closures = BTreeMap::new();
    for rule in rules {
        closures.insert(rule.package.to_string(), graph.closure(rule.package)?);
    }
    Ok(closures)
}

/// Read `cargo metadata --format-version 1 --no-deps` (manifest only).
fn metadata(root: &Path) -> Result<Value, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline"])
        .output()
        .map_err(|error| format!("run `cargo metadata`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "cargo metadata: output is not UTF-8".to_string())?;
    json::parse(&text).map_err(|error| format!("cargo metadata: {error}"))
}

fn contract_path(root: &Path) -> std::path::PathBuf {
    root.join(CONTRACT)
}

fn render(closures: &BTreeMap<String, Closure>) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"schema\": 1,\n");
    out.push_str("  \"generated_by\": \"cargo xtask editions --write\",\n");
    out.push_str(
        "  \"note\": \"Transitive normal dependency closure per edition (workspace crates \
         plus opencascade*), excluding dev- and build-dependencies. Regenerate with \
         `cargo xtask editions --write`; `--check` fails on drift.\",\n",
    );
    out.push_str("  \"editions\": {\n");
    let count = closures.len();
    for (index, (edition, closure)) in closures.iter().enumerate() {
        out.push_str(&format!("    {}: [\n", json::quote(edition)));
        let total = closure.crates.len();
        for (position, krate) in closure.crates.iter().enumerate() {
            out.push_str(&format!("      {}", json::quote(krate)));
            if position + 1 < total {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("    ]");
        if index + 1 < count {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  }\n");
    out.push_str("}\n");
    out
}

fn check_contract(root: &Path, closures: &BTreeMap<String, Closure>) -> Result<(), String> {
    let path = contract_path(root);
    let text = fs::read_to_string(&path).map_err(|error| {
        format!(
            "read {}: {error}; run `cargo xtask editions --write`",
            path.display()
        )
    })?;
    let metadata = json::parse(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
    let pinned = metadata
        .field("editions")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{}: no `editions` object", path.display()))?;

    let mut problems = Vec::new();
    for (edition, closure) in closures {
        let Some((_, value)) = pinned.iter().find(|(name, _)| name == edition) else {
            problems.push(format!("`{edition}`: missing from the contract"));
            continue;
        };
        let recorded: BTreeSet<String> = strings(value)?.into_iter().collect();
        let added: Vec<&String> = closure.crates.difference(&recorded).collect();
        let removed: Vec<&String> = recorded.difference(&closure.crates).collect();
        if !added.is_empty() || !removed.is_empty() {
            problems.push(format!(
                "`{edition}`: closure changed (added: {}; removed: {})",
                list(&added),
                list(&removed)
            ));
        }
    }
    for (edition, _) in pinned {
        if !closures.contains_key(edition) {
            problems.push(format!("`{edition}`: pinned but no longer an edition"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} is stale:\n  {}\nrun `cargo xtask editions --write` and review the diff",
            CONTRACT,
            problems.join("\n  ")
        ))
    }
}

fn list(items: &[&String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("`{item}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `cargo xtask editions [--check|--write]`.
pub fn task(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut write = false;
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--write" => write = true,
            "--check" => check = true,
            other => {
                return Err(format!(
                    "unknown flag `{other}`; expected `--check` or `--write`"
                ));
            }
        }
    }
    if write && check {
        return Err("pass either `--check` or `--write`, not both".to_string());
    }
    let root = crate::workspace_root();
    let graph = Graph::from_metadata(&metadata(&root)?)?;
    let closures = compute(&graph, RULES)?;
    let violations = check_rules(&closures, RULES)?;

    if write {
        if !violations.is_empty() {
            return Err(format!(
                "refusing to write {}: the graph breaks {} rule(s)\n  {}",
                CONTRACT,
                violations.len(),
                violations.join("\n  ")
            ));
        }
        let path = contract_path(&root);
        fs::write(&path, render(&closures))
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        println!("wrote {} ({} editions)", CONTRACT, closures.len());
        return Ok(());
    }

    if !violations.is_empty() {
        return Err(format!(
            "edition dependency rules failed:\n  {}",
            violations.join("\n  ")
        ));
    }
    check_contract(&root, &closures)?;
    for (edition, closure) in &closures {
        let count = closure.crates.len();
        let noun = if count == 1 { "crate" } else { "crates" };
        println!("{edition}: {count} {noun}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Closure, Graph, KERNEL_FREE, RULES, Rule, check_rules, compute, offending_route, render,
    };

    /// A synthetic metadata document: dev, build, optional and renamed edges, and
    /// `ferrocad_occt` two hops out from the edition with `opencascade-sys` below
    /// it.
    const SYNTHETIC: &str = r#"{
      "packages": [
        {
          "name": "root",
          "features": {"default": ["dep:extra"], "unused": []},
          "dependencies": [
            {"name": "mid", "kind": null, "rename": null, "optional": false,
             "uses_default_features": true, "features": []},
            {"name": "ferrocad_part", "kind": "dev", "rename": null, "optional": false,
             "uses_default_features": true, "features": []},
            {"name": "ferrocad_mesh", "kind": "build", "rename": null, "optional": false,
             "uses_default_features": true, "features": []},
            {"name": "ferrocad_curves", "kind": null, "rename": "extra", "optional": true,
             "uses_default_features": true, "features": []},
            {"name": "ferrocad_mesh_py", "kind": null, "rename": null, "optional": true,
             "uses_default_features": true, "features": []}
          ]
        },
        {
          "name": "mid",
          "features": {},
          "dependencies": [
            {"name": "ferrocad_occt", "kind": null, "rename": null, "optional": false,
             "uses_default_features": true, "features": []}
          ]
        },
        {
          "name": "ferrocad_occt",
          "features": {},
          "dependencies": [
            {"name": "opencascade-sys", "kind": null, "rename": null, "optional": false,
             "uses_default_features": true, "features": []}
          ]
        },
        {"name": "ferrocad_curves", "features": {}, "dependencies": []},
        {"name": "ferrocad_part", "features": {}, "dependencies": []},
        {"name": "ferrocad_mesh", "features": {}, "dependencies": []},
        {"name": "ferrocad_mesh_py", "features": {}, "dependencies": []}
      ]
    }"#;

    fn graph(source: &str) -> Graph {
        Graph::from_metadata(&crate::json::parse(source).expect("test metadata parses"))
            .expect("graph")
    }

    fn names(closure: &Closure) -> Vec<&str> {
        closure.crates.iter().map(String::as_str).collect()
    }

    #[test]
    fn walks_only_normal_and_enabled_dependencies() {
        let graph = graph(SYNTHETIC);
        let closure = graph.closure("root").expect("closure");
        assert_eq!(
            names(&closure),
            ["ferrocad_curves", "ferrocad_occt", "mid", "opencascade-sys", "root"]
        );
        // The dev-dep `ferrocad_part`, the build-dep `ferrocad_mesh` and the
        // optional, feature-less `ferrocad_mesh_py` are all absent.
        for absent in ["ferrocad_part", "ferrocad_mesh", "ferrocad_mesh_py"] {
            assert!(!closure.crates.contains(absent), "{absent} leaked in");
        }
    }

    #[test]
    fn finds_the_kernel_two_hops_away_and_reports_the_route() {
        let graph = graph(SYNTHETIC);
        let closure = graph.closure("root").expect("closure");
        assert_eq!(
            offending_route(&closure, "root", "opencascade-sys"),
            Some(vec![
                "root".to_string(),
                "mid".to_string(),
                "ferrocad_occt".to_string(),
                "opencascade-sys".to_string(),
            ])
        );
    }

    #[test]
    fn question_mark_does_not_enable_an_optional_dependency() {
        let disabled = graph(
            r#"{"packages":[
                 {"name":"root","features":{"default":["mid?/fast"]},
                  "dependencies":[
                    {"name":"mid","kind":null,"rename":null,"optional":true,
                     "uses_default_features":false,"features":[]}]},
                 {"name":"mid","features":{"fast":["dep:ferrocad_occt"]},
                  "dependencies":[
                    {"name":"ferrocad_occt","kind":null,"rename":null,"optional":true,
                     "uses_default_features":true,"features":[]}]},
                 {"name":"ferrocad_occt","features":{},"dependencies":[]}]}"#,
        );
        let closure = disabled.closure("root").expect("closure");
        assert_eq!(names(&closure), ["root"]);

        // The same edge without `?` does enable it, and only then does the
        // requested feature switch its optional dependency on.
        let enabled = graph(
            r#"{"packages":[
                 {"name":"root","features":{"default":["mid/fast"]},
                  "dependencies":[
                    {"name":"mid","kind":null,"rename":null,"optional":true,
                     "uses_default_features":false,"features":[]}]},
                 {"name":"mid","features":{"fast":["dep:ferrocad_occt"]},
                  "dependencies":[
                    {"name":"ferrocad_occt","kind":null,"rename":null,"optional":true,
                     "uses_default_features":true,"features":[]}]},
                 {"name":"ferrocad_occt","features":{},"dependencies":[]}]}"#,
        );
        let closure = enabled.closure("root").expect("closure");
        assert_eq!(names(&closure), ["ferrocad_occt", "mid", "root"]);
    }

    #[test]
    fn disabled_default_features_do_not_activate_the_dependency_defaults() {
        let graph = graph(
            r#"{"packages":[
                 {"name":"root","features":{},
                  "dependencies":[
                    {"name":"mid","kind":null,"rename":null,"optional":false,
                     "uses_default_features":false,"features":[]}]},
                 {"name":"mid","features":{"default":["dep:ferrocad_occt"]},
                  "dependencies":[
                    {"name":"ferrocad_occt","kind":null,"rename":null,"optional":true,
                     "uses_default_features":true,"features":[]}]},
                 {"name":"ferrocad_occt","features":{},"dependencies":[]}]}"#,
        );
        let closure = graph.closure("root").expect("closure");
        assert_eq!(names(&closure), ["mid", "root"]);
    }

    #[test]
    fn reports_a_forbidden_crate_with_the_offending_rule_and_path() {
        let graph = graph(
            &SYNTHETIC.replace("\"name\": \"root\"", "\"name\": \"ferrocad_toy\""),
        );
        let rules = &[Rule {
            package: "ferrocad_toy",
            reason: "the toy edition is deliberately kernel-free",
            forbidden: KERNEL_FREE,
            required: &[],
        }];
        let closures = compute(&graph, rules).expect("closures");
        let violations = check_rules(&closures, rules).expect("check");
        assert_eq!(violations.len(), 3, "{violations:#?}");
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("`ferrocad_toy` must not reach `ferrocad_curves`"))
        );
        let occt = violations
            .iter()
            .find(|violation| violation.contains("`ferrocad_toy` must not reach `ferrocad_occt`"))
            .expect("occt violation");
        assert!(occt.contains("path: ferrocad_toy -> mid -> ferrocad_occt"), "{occt}");
        let sys = violations
            .iter()
            .find(|violation| violation.contains("`ferrocad_toy` must not reach `opencascade-sys`"))
            .expect("kernel violation");
        assert!(
            sys.contains("path: ferrocad_toy -> mid -> ferrocad_occt -> opencascade-sys"),
            "{sys}"
        );
    }

    #[test]
    fn the_geometry_seam_is_the_one_allowed_route_to_pure_geometry() {
        let graph = graph(
            r#"{"packages":[
                 {"name":"ferrocad_toy","features":{},
                  "dependencies":[
                    {"name":"ferrocad_geom","kind":null,"rename":null,"optional":false,
                     "uses_default_features":true,"features":[]}]},
                 {"name":"ferrocad_geom","features":{},
                  "dependencies":[
                    {"name":"ferrocad_curves","kind":null,"rename":null,"optional":false,
                     "uses_default_features":true,"features":[]}]},
                 {"name":"ferrocad_curves","features":{},"dependencies":[]}]}"#,
        );
        let rules = &[Rule {
            package: "ferrocad_toy",
            reason: "the toy edition is deliberately kernel-free",
            forbidden: KERNEL_FREE,
            required: &[],
        }];
        let closures = compute(&graph, rules).expect("closures");
        assert!(
            closures["ferrocad_toy"]
                .crates
                .contains("ferrocad_curves")
        );
        assert!(check_rules(&closures, rules).expect("check").is_empty());

        // A second, seam-less route is a violation.
        let mut edges = vec![
            ("ferrocad_toy".to_string(), "ferrocad_curves".to_string()),
            ("ferrocad_curves".to_string(), "ferrocad_mesh".to_string()),
        ];
        edges.sort();
        let closure = Closure {
            crates: [
                "ferrocad_curves",
                "ferrocad_geom",
                "ferrocad_mesh",
                "ferrocad_toy",
            ]
            .iter()
            .map(|name| name.to_string())
            .collect(),
            edges,
        };
        assert_eq!(
            offending_route(&closure, "ferrocad_toy", "ferrocad_curves"),
            Some(vec!["ferrocad_toy".to_string(), "ferrocad_curves".to_string()])
        );
        assert_eq!(
            offending_route(&closure, "ferrocad_toy", "ferrocad_mesh"),
            Some(vec![
                "ferrocad_toy".to_string(),
                "ferrocad_curves".to_string(),
                "ferrocad_mesh".to_string()
            ])
        );
    }

    #[test]
    fn reports_a_missing_required_crate() {
        let graph = graph(&SYNTHETIC.replace("\"name\": \"root\"", "\"name\": \"ferrocad_parametric\""));
        let rules = &[Rule {
            package: "ferrocad_parametric",
            reason: "the parametric edition links the Part workbench",
            forbidden: &[],
            required: &["ferrocad_part_py", "ferrocad_occt"],
        }];
        let closures = compute(&graph, rules).expect("closures");
        let violations = check_rules(&closures, rules).expect("check");
        assert_eq!(violations.len(), 1, "{violations:#?}");
        assert!(violations[0].contains("`ferrocad_parametric` must reach `ferrocad_part_py`"));
    }

    #[test]
    fn the_real_rules_cover_every_edition() {
        let editions: Vec<&str> = RULES.iter().map(|rule| rule.package).collect();
        for edition in [
            "ferrocad",
            "ferrocad_toy",
            "ferrocad_gpui",
            "ferrocad_stage",
            "ferrocad_viewport",
            "ferrocad_widgets",
            "ferrocad_inventor",
            "ferrocad_wgpu",
            "ferrocad_ui",
            "ferrocad_py",
            "ferrocad_parametric",
            "ferrocad_combined_py",
        ] {
            assert!(editions.contains(&edition), "no rule for {edition}");
        }
        assert!(KERNEL_FREE.contains(&"opencascade-sys"));
        assert!(KERNEL_FREE.contains(&"ferrocad_occt"));
    }

    #[test]
    fn contract_round_trips() {
        let graph = graph(SYNTHETIC);
        let rules = &[Rule {
            package: "root",
            reason: "test edition",
            forbidden: &[],
            required: &[],
        }];
        let closures = compute(&graph, rules).expect("closures");
        let text = render(&closures);
        let parsed = crate::json::parse(&text).expect("render parses back");
        let editions = parsed
            .field("editions")
            .and_then(crate::json::Value::as_object)
            .expect("editions");
        let recorded: Vec<&str> = editions[0]
            .1
            .as_array()
            .expect("array")
            .iter()
            .map(|item| item.as_str().expect("string"))
            .collect();
        assert_eq!(recorded, names(&closures["root"]));
    }
}
