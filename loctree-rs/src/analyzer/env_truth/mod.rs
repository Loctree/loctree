//! Cut 8 (P0) — `loct env-truth`: declaration-side env audit.
//!
//! Lane 4 of the LOCTREE_NEXT.md doctrine. Surfaces **where** env vars are
//! declared (dotenv, dockerfile, docker-compose, k8s, helm, GHA, npm,
//! tauri.conf, sops markers) and cross-references the read side already
//! produced by Cut 3B (`semantic_facts.env_contracts`).
//!
//! Single rule that we never break: encrypted/sealed payloads are surfaced
//! by **format markers only** — no decoding ever, even when local keys
//! could in principle do it.
//!
//! See `docs/env-truth-precedence.md` for the precedence-rank doctrine.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::pack::AuthorityLabel;
use crate::semantic::SemanticFacts;
use crate::snapshot::Snapshot;

mod discovery;
mod docker_compose;
mod dockerfile;
mod dotenv;
mod gha;
mod helm;
mod io_helpers;
mod k8s;
mod npm_script;
mod precedence;
mod sops_marker;
mod types;
mod warnings;

pub use precedence::EnvTruthConfig;
pub use types::{
    ENV_TRUTH_SCHEMA_VERSION, EnvDeclaration, EnvReadSite, EnvSource, EnvSourceKind,
    EnvTruthReport, EnvTruthSummary, EnvWarning, FailOnKind, OrphanRead, ScanRoot, TemplateDrift,
    ValuePresence,
};
pub use warnings::DEFAULT_STALE_THRESHOLD_DAYS;

/// Inputs to [`compute_env_truth`]. Constructed from CLI options.
#[derive(Debug, Clone)]
pub struct ComputeConfig {
    /// Scan roots (absolute paths preferred). Empty means current dir.
    pub roots: Vec<PathBuf>,
    /// Optional path-restriction set (relative to root).
    pub restricted_paths: Vec<PathBuf>,
    /// Optional config-file overrides for the precedence table. Loaded by
    /// the caller (handler) before invoking compute_env_truth.
    pub precedence_override: Option<EnvTruthConfig>,
    /// Stale-overrides-fresh threshold in days. Defaults to
    /// [`warnings::DEFAULT_STALE_THRESHOLD_DAYS`].
    pub stale_threshold_days: u32,
    /// Suppress stderr noise.
    pub quiet: bool,
}

impl Default for ComputeConfig {
    fn default() -> Self {
        Self {
            roots: vec![PathBuf::from(".")],
            restricted_paths: Vec::new(),
            precedence_override: None,
            stale_threshold_days: warnings::DEFAULT_STALE_THRESHOLD_DAYS,
            quiet: false,
        }
    }
}

/// Top-level entry point. Builds the full [`EnvTruthReport`].
///
/// `snapshot` is optional — when present its `semantic_facts.env_contracts`
/// populate the read-side; when absent, declarations stand alone and orphan
/// detection is skipped (we cannot tell read-but-undeclared apart from
/// "scanner didn't run yet").
pub fn compute_env_truth(config: &ComputeConfig, snapshot: Option<&Snapshot>) -> EnvTruthReport {
    let mut table = precedence::default_table();
    if let Some(cfg) = &config.precedence_override {
        precedence::apply_override(&mut table, cfg, config.quiet);
    }

    let candidates = discovery::discover_candidates(&config.roots, &config.restricted_paths);
    let scan_root = config
        .roots
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."));

    let mut bucket: HashMap<String, Vec<EnvSource>> = HashMap::new();
    let mut env_file_chain: Vec<PathBuf> = Vec::new();

    // TEMPLATE fence (W2-c): `.env.example` / `*.sample` / `*.template` are
    // shapes, never live sources. They are routed away from the precedence
    // ranking entirely and compared key-by-key in template-drift mode below.
    let mut template_keys: Vec<(String, Vec<String>)> = Vec::new();

    // Sensors — order doesn't matter for correctness; sources merge by name.
    for path in &candidates.dotenv {
        let pairs = dotenv::parse_dotenv_file(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::DotEnv).unwrap_or(&30),
            false,
        );
        let rel = path
            .strip_prefix(&scan_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        if precedence::is_template_path(&rel) {
            template_keys.push((rel, pairs.into_iter().map(|(name, _)| name).collect()));
            continue;
        }
        push_pairs(&mut bucket, pairs);
    }
    for path in &candidates.envrc {
        let pairs = dotenv::parse_envrc_file(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::EnvRc).unwrap_or(&8),
        );
        push_pairs(&mut bucket, pairs);
    }
    for path in &candidates.dockerfile {
        let pairs = dockerfile::parse_dockerfile(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::Dockerfile).unwrap_or(&40),
        );
        push_pairs(&mut bucket, pairs);
    }
    for path in &candidates.docker_compose {
        let (pairs, refs) = docker_compose::parse_compose_file(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::DockerCompose).unwrap_or(&50),
        );
        push_pairs(&mut bucket, pairs);
        env_file_chain.extend(refs);
    }
    // Resolve env_file: chain references — parse those dotenv files at the
    // DockerComposeEnvFile rank so they don't merge with regular dotenvs.
    for path in &env_file_chain {
        if !path.exists() {
            continue;
        }
        let pairs = dotenv::parse_dotenv_file(
            path,
            &scan_root,
            *table
                .get(&EnvSourceKind::DockerComposeEnvFile)
                .unwrap_or(&45),
            false,
        );
        let pairs = pairs
            .into_iter()
            .map(|(name, mut src)| {
                src.kind = EnvSourceKind::DockerComposeEnvFile;
                (name, src)
            })
            .collect::<Vec<_>>();
        push_pairs(&mut bucket, pairs);
    }
    for path in &candidates.k8s_yaml {
        let pairs = k8s::parse_k8s_yaml(path, &scan_root, &table);
        push_pairs(&mut bucket, pairs);
    }
    for path in &candidates.helm_values {
        let pairs = helm::parse_values_file(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::HelmValues).unwrap_or(&65),
        );
        push_pairs(&mut bucket, pairs);
    }
    // Workflow declarations + shell `run:` reads. Reads collected here are
    // merged into `read_index` below so an env var declared in the same
    // workflow's `env:` block does not trip a false `orphan-declaration`
    // warning when only referenced via `$VAR` / `${VAR}` in shell steps.
    let mut workflow_shell_reads: HashMap<String, Vec<EnvReadSite>> = HashMap::new();
    for path in &candidates.github_workflows {
        let pairs = gha::parse_workflow_file(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::GitHubActionsEnv).unwrap_or(&15),
            *table
                .get(&EnvSourceKind::GitHubActionsSecret)
                .unwrap_or(&20),
        );
        push_pairs(&mut bucket, pairs);
        for (name, site) in gha::parse_workflow_shell_reads(path, &scan_root) {
            workflow_shell_reads.entry(name).or_default().push(site);
        }
    }
    for path in &candidates.sops_files {
        if let Some(pair) = sops_marker::parse_sops_file(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::SopsFile).unwrap_or(&78),
        ) {
            push_pairs(&mut bucket, vec![pair]);
        }
    }
    for path in &candidates.npm_packages {
        let pairs = npm_script::parse_package_json(
            path,
            &scan_root,
            *table.get(&EnvSourceKind::NpmScript).unwrap_or(&35),
        );
        push_pairs(&mut bucket, pairs);
    }

    // Drop synthetic markers (`__env_file__`, `__sops__`, `__env_from__`)
    // — they exist to keep the orchestrator wiring honest but should not
    // appear in the user-facing report.
    bucket.retain(|name, _| !is_synthetic(name));

    // Cross-ref read sites from semantic_facts.env_contracts.
    let mut read_index = build_read_index(snapshot.and_then(|s| s.semantic_facts.as_ref()));
    // Merge in-workflow shell `$VAR` references so cross-block reads in
    // the same workflow file count as legitimate reads.
    for (name, sites) in workflow_shell_reads {
        read_index.entry(name).or_default().extend(sites);
    }
    let has_env_contracts = snapshot
        .and_then(|s| s.semantic_facts.as_ref())
        .map(|f| !f.env_contracts.is_empty())
        .unwrap_or(false);

    // Assemble declarations (sorted by name for stable output).
    let mut declarations: Vec<EnvDeclaration> = Vec::new();
    let mut all_names: std::collections::BTreeSet<String> = bucket.keys().cloned().collect();
    for name in read_index.keys() {
        all_names.insert(name.clone());
    }
    for name in &all_names {
        let mut sources = bucket.remove(name).unwrap_or_default();
        // Sort sources by precedence_rank descending — highest-precedence first.
        sources.sort_by(|a, b| {
            b.precedence_rank
                .cmp(&a.precedence_rank)
                .then_with(|| a.path.cmp(&b.path))
        });
        let reads = read_index.get(name).cloned().unwrap_or_default();
        let authority = if !sources.is_empty() {
            AuthorityLabel::SemanticGuess
        } else {
            AuthorityLabel::StaleOrUnknown
        };
        let mut decl = EnvDeclaration {
            name: name.clone(),
            sources,
            reads: reads.clone(),
            precedence_warnings: Vec::new(),
            authority,
        };
        warnings::compute_warnings(&mut decl, has_env_contracts, config.stale_threshold_days);
        declarations.push(decl);
    }

    // Build top-level orphan_reads (mirror of OrphanCodeReference).
    // Runtime-provided names (OS env, shell/CI builtins) are exempt —
    // same predicate as the per-declaration warning.
    let orphan_reads: Vec<OrphanRead> = declarations
        .iter()
        .filter(|d| {
            d.sources.is_empty()
                && !d.reads.is_empty()
                && !warnings::is_runtime_provided_env(&d.name)
        })
        .map(|d| OrphanRead {
            name: d.name.clone(),
            read_sites: d.reads.clone(),
        })
        .collect();

    // Template drift: compare each template's promised keys against the
    // live declaration set. Missing-in-live = template promises a key no
    // live source declares; extra-in-live = a live dotenv in the template's
    // directory declares a key the template omits (stale documentation).
    let names_with_sources: std::collections::BTreeSet<&str> = declarations
        .iter()
        .filter(|d| !d.sources.is_empty())
        .map(|d| d.name.as_str())
        .collect();
    let mut template_drift: Vec<TemplateDrift> = Vec::new();
    for (template_path, keys) in &template_keys {
        let template_dir = Path::new(template_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let key_set: std::collections::BTreeSet<&str> = keys.iter().map(|k| k.as_str()).collect();
        let missing_in_live: Vec<String> = keys
            .iter()
            .filter(|k| !names_with_sources.contains(k.as_str()))
            .cloned()
            .collect();
        let mut extra_in_live: Vec<String> = declarations
            .iter()
            .filter(|d| {
                !key_set.contains(d.name.as_str())
                    && d.sources.iter().any(|s| {
                        s.kind == EnvSourceKind::DotEnv
                            && Path::new(&s.path)
                                .parent()
                                .map(|p| p.to_string_lossy() == template_dir.as_str())
                                .unwrap_or(false)
                    })
            })
            .map(|d| d.name.clone())
            .collect();
        extra_in_live.sort();
        if !missing_in_live.is_empty() || !extra_in_live.is_empty() {
            template_drift.push(TemplateDrift {
                template_path: template_path.clone(),
                missing_in_live,
                extra_in_live,
            });
        }
    }
    template_drift.sort_by(|a, b| a.template_path.cmp(&b.template_path));

    // Summary.
    let mut warnings_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_sources = 0usize;
    for decl in &declarations {
        total_sources += decl.sources.len();
        for w in &decl.precedence_warnings {
            let key = warning_kind(w);
            *warnings_by_kind.entry(key.into()).or_insert(0) += 1;
        }
    }
    if !template_drift.is_empty() {
        warnings_by_kind.insert("template_drift".into(), template_drift.len());
    }
    let mut precedence_table_out: BTreeMap<String, u8> = BTreeMap::new();
    for (kind, rank) in &table {
        precedence_table_out.insert(kind_to_key(*kind).into(), *rank);
    }
    let summary = EnvTruthSummary {
        total_declarations: declarations.len(),
        total_sources,
        orphan_reads: orphan_reads.len(),
        warnings_by_kind,
        precedence_table: precedence_table_out,
    };

    let roots: Vec<String> = config
        .roots
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    EnvTruthReport {
        schema_version: ENV_TRUTH_SCHEMA_VERSION.into(),
        generated_at: now_rfc3339(),
        roots,
        declarations,
        orphan_reads,
        template_drift,
        summary,
    }
}

fn push_pairs(bucket: &mut HashMap<String, Vec<EnvSource>>, pairs: Vec<(String, EnvSource)>) {
    for (name, source) in pairs {
        bucket.entry(name).or_default().push(source);
    }
}

fn is_synthetic(name: &str) -> bool {
    name.starts_with("__") && name.ends_with("__")
}

fn build_read_index(facts: Option<&SemanticFacts>) -> HashMap<String, Vec<EnvReadSite>> {
    let mut out: HashMap<String, Vec<EnvReadSite>> = HashMap::new();
    let Some(facts) = facts else { return out };
    for contract in &facts.env_contracts {
        if is_makefile_assignment_contract(contract) {
            continue;
        }
        let mut sites = Vec::new();
        for file in &contract.used_in_files {
            sites.push(EnvReadSite {
                file: file.clone(),
                line: None,
                symbol: None,
                required_for: contract.required_for.clone(),
            });
        }
        out.insert(contract.name.clone(), sites);
    }
    out
}

fn is_makefile_assignment_contract(contract: &crate::semantic::EnvContract) -> bool {
    contract
        .required_for
        .iter()
        .any(|reason| reason == "Makefile variable assignment")
}

fn warning_kind(w: &EnvWarning) -> &'static str {
    match w {
        EnvWarning::StaleOverridesFresh { .. } => "stale_overrides_fresh",
        EnvWarning::MultiSourceValueMismatch { .. } => "multi_source_value_mismatch",
        EnvWarning::OrphanCodeReference { .. } => "orphan_code_reference",
        EnvWarning::OrphanDeclaration { .. } => "orphan_declaration",
        EnvWarning::SealedSecretSuspectedStale { .. } => "sealed_secret_suspected_stale",
        EnvWarning::EncryptedDecodeBlocked { .. } => "encrypted_decode_blocked",
    }
}

fn kind_to_key(kind: EnvSourceKind) -> &'static str {
    use EnvSourceKind::*;
    match kind {
        DotEnv => "dot_env",
        EnvRc => "env_rc",
        Dockerfile => "dockerfile",
        DockerCompose => "docker_compose",
        DockerComposeEnvFile => "docker_compose_env_file",
        K8sDeploymentEnv => "k8s_deployment_env",
        K8sDeploymentEnvFrom => "k8s_deployment_env_from",
        K8sConfigMap => "k8s_config_map",
        K8sSecret => "k8s_secret",
        K8sSecretStringData => "k8s_secret_string_data",
        SealedSecret => "sealed_secret",
        ExternalSecret => "external_secret",
        SopsFile => "sops_file",
        HelmValues => "helm_values",
        GitHubActionsEnv => "github_actions_env",
        GitHubActionsSecret => "github_actions_secret",
        NpmScript => "npm_script",
        TauriConf => "tauri_conf",
    }
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

/// Rendering knobs for [`render_markdown`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOptions {
    /// Full per-declaration dump (`--all`). Default is the "Top problems"
    /// view: real conflicts + template drift + capped orphan lists — no
    /// 2026-line walls.
    pub all: bool,
    /// Show `sha256:` value hashes (`--hashes`). Hidden by default — the
    /// hash reads like a value at a glance and is only useful when
    /// comparing sources by eye.
    pub show_hashes: bool,
}

/// Cap for orphan lists in the default (Top-problems) view.
const TOP_ORPHAN_CAP: usize = 15;

/// Render an [`EnvTruthReport`] as Markdown. Operator runbook-friendly:
/// h1 / h2 hierarchy, tables, precedence chain shown high-to-low.
///
/// Default view = "Top problems": multi-source conflicts, stale-overrides,
/// template drift, and capped orphan summaries. `opts.all` restores the
/// full per-declaration dump.
pub fn render_markdown(report: &EnvTruthReport, opts: &RenderOptions) -> String {
    let mut md = String::new();
    md.push_str("# loct env-truth report\n\n");
    md.push_str(&format!(
        "_Generated: {} · schema {} · roots: {}_\n\n",
        report.generated_at,
        report.schema_version,
        report.roots.join(", ")
    ));

    md.push_str("## Summary\n\n");
    md.push_str(&format!(
        "- Declarations: **{}**\n- Sources: **{}**\n- Orphan reads: **{}**\n",
        report.summary.total_declarations,
        report.summary.total_sources,
        report.summary.orphan_reads
    ));
    if !report.summary.warnings_by_kind.is_empty() {
        md.push_str("\n### Warnings by kind\n\n");
        md.push_str("| Kind | Count |\n|---|---|\n");
        for (k, v) in &report.summary.warnings_by_kind {
            md.push_str(&format!("| `{k}` | {v} |\n"));
        }
    }

    if opts.all {
        render_declarations_full(report, opts, &mut md);
    } else {
        render_top_problems(report, opts, &mut md);
    }

    render_template_drift(report, &mut md);

    if !report.orphan_reads.is_empty() {
        md.push_str("\n## Orphan code references (read but never declared)\n\n");
        let cap = if opts.all {
            report.orphan_reads.len()
        } else {
            TOP_ORPHAN_CAP
        };
        for o in report.orphan_reads.iter().take(cap) {
            let sites: Vec<&str> = o.read_sites.iter().map(|r| r.file.as_str()).collect();
            md.push_str(&format!("- `{}` — read in: {}\n", o.name, sites.join(", ")));
        }
        if report.orphan_reads.len() > cap {
            md.push_str(&format!(
                "- _… and {} more (run with `--all` for the full list)_\n",
                report.orphan_reads.len() - cap
            ));
        }
    }

    if opts.all {
        md.push_str("\n## Precedence table (active)\n\n");
        md.push_str("| Source kind | Rank |\n|---|---:|\n");
        for (k, r) in &report.summary.precedence_table {
            md.push_str(&format!("| `{k}` | {r} |\n"));
        }
        md.push('\n');
        md.push_str("> See `docs/env-truth-precedence.md` for the doctrine. Override via `.loctree/config.toml [env_truth] precedence = ...`.\n");
    } else {
        md.push_str(
            "\n> Top-problems view. Run `loct env-truth --all` for the full declaration dump, `--hashes` for value hashes.\n",
        );
    }

    md
}

/// Conflict kinds that earn a place in the default "Top problems" view.
fn is_top_problem_warning(w: &EnvWarning) -> bool {
    matches!(
        w,
        EnvWarning::MultiSourceValueMismatch { .. }
            | EnvWarning::StaleOverridesFresh { .. }
            | EnvWarning::SealedSecretSuspectedStale { .. }
    )
}

fn render_top_problems(report: &EnvTruthReport, opts: &RenderOptions, md: &mut String) {
    let problem_decls: Vec<&EnvDeclaration> = report
        .declarations
        .iter()
        .filter(|d| d.precedence_warnings.iter().any(is_top_problem_warning))
        .collect();

    md.push_str("\n## Top problems\n\n");
    if problem_decls.is_empty() {
        md.push_str("_No multi-source conflicts or stale-precedence issues found._\n");
    }
    for decl in &problem_decls {
        render_declaration(decl, opts, md);
    }

    // Orphan declarations: compact name list, not one heading per var.
    let orphan_decl_names: Vec<&str> = report
        .declarations
        .iter()
        .filter(|d| {
            d.precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::OrphanDeclaration { .. }))
        })
        .map(|d| d.name.as_str())
        .collect();
    if !orphan_decl_names.is_empty() {
        md.push_str("\n## Orphan declarations (declared but never read)\n\n");
        let shown: Vec<&str> = orphan_decl_names
            .iter()
            .take(TOP_ORPHAN_CAP)
            .copied()
            .collect();
        md.push_str(&format!("`{}`", shown.join("`, `")));
        if orphan_decl_names.len() > TOP_ORPHAN_CAP {
            md.push_str(&format!(
                " _… and {} more_",
                orphan_decl_names.len() - TOP_ORPHAN_CAP
            ));
        }
        md.push('\n');
    }
}

fn render_declarations_full(report: &EnvTruthReport, opts: &RenderOptions, md: &mut String) {
    md.push_str("\n## Declarations\n\n");
    if report.declarations.is_empty() {
        md.push_str("_No env declarations or reads discovered in scope._\n");
    }
    for decl in &report.declarations {
        render_declaration(decl, opts, md);
    }
}

fn render_declaration(decl: &EnvDeclaration, opts: &RenderOptions, md: &mut String) {
    md.push_str(&format!("### `{}`\n\n", decl.name));
    if !decl.sources.is_empty() {
        md.push_str("| Rank | Kind | Path | Line | mtime | Age | Value |\n");
        md.push_str("|---:|---|---|---:|---|---:|---|\n");
        for s in &decl.sources {
            let age = s
                .mtime_age_days
                .map(|d| format!("{d}d"))
                .unwrap_or_else(|| "?".into());
            let line = s.line.map(|l| l.to_string()).unwrap_or_else(|| "-".into());
            md.push_str(&format!(
                "| {} | {:?} | `{}` | {} | {} | {} | {} |\n",
                s.precedence_rank,
                s.kind,
                s.path,
                line,
                s.mtime,
                age,
                render_value_presence(&s.value_present, opts.show_hashes)
            ));
        }
    }
    if !decl.reads.is_empty() {
        md.push_str("\n**Reads:**\n");
        for r in &decl.reads {
            md.push_str(&format!("- `{}`\n", r.file));
        }
    }
    if !decl.precedence_warnings.is_empty() {
        md.push_str("\n**Warnings:**\n");
        for w in &decl.precedence_warnings {
            md.push_str(&format!("- {}\n", render_warning(w)));
        }
    }
    md.push('\n');
}

fn render_template_drift(report: &EnvTruthReport, md: &mut String) {
    if report.template_drift.is_empty() {
        return;
    }
    md.push_str("\n## Template drift\n\n");
    for drift in &report.template_drift {
        md.push_str(&format!("### `{}`\n\n", drift.template_path));
        if !drift.missing_in_live.is_empty() {
            md.push_str(&format!(
                "- **missing in live env** (template promises, nothing declares): `{}`\n",
                drift.missing_in_live.join("`, `")
            ));
        }
        if !drift.extra_in_live.is_empty() {
            md.push_str(&format!(
                "- **missing in template** (live env declares, template omits): `{}`\n",
                drift.extra_in_live.join("`, `")
            ));
        }
    }
}

fn render_value_presence(v: &ValuePresence, show_hashes: bool) -> String {
    match v {
        // Label EXPLICITLY as a hash so operators do not read the SHA-256
        // first-12-hex as if it were the literal value. Past incident
        // (Screenscribe 2026-05-18): `HEALTH_THRESHOLD: 50` rendered as
        // `value: plain '1a6562590ef1'`, operator concluded the value was
        // `1a6562590ef1`. The bytes never lied — the *label* did.
        // W2-c: the hash now hides behind `--hashes` entirely; the default
        // label is a bare `plain`.
        ValuePresence::Plain { value_hash } => {
            if show_hashes {
                format!("plain hash `sha256:{value_hash}`")
            } else {
                "plain".into()
            }
        }
        ValuePresence::Encrypted { marker } => format!("**encrypted** ({marker})"),
        ValuePresence::EnvFrom { reference } => format!("envFrom `{reference}`"),
        ValuePresence::Secret => "**k8s secret** (base64, never decoded)".into(),
        ValuePresence::Empty => "_empty_".into(),
    }
}

fn render_warning(w: &EnvWarning) -> String {
    match w {
        EnvWarning::StaleOverridesFresh {
            stale_source,
            fresh_source,
            age_delta_days,
        } => format!(
            "stale-overrides-fresh: `{}` (stale) overrides `{}` (fresh) by {} days",
            stale_source, fresh_source, age_delta_days
        ),
        EnvWarning::MultiSourceValueMismatch { sources } => format!(
            "multi-source-value-mismatch across [{}]",
            sources.join(", ")
        ),
        EnvWarning::OrphanCodeReference { read_sites } => format!(
            "orphan-code-reference: read in [{}] but no declaration found",
            read_sites.join(", ")
        ),
        EnvWarning::OrphanDeclaration { sources } => {
            format!(
                "orphan-declaration: declared in [{}] but never read",
                sources.join(", ")
            )
        }
        EnvWarning::SealedSecretSuspectedStale {
            sealed_path,
            sealed_age_days,
            plain_age_days,
        } => format!(
            "sealed-secret-suspected-stale: `{}` is {}d old vs plain {}d — example-app pattern",
            sealed_path, sealed_age_days, plain_age_days
        ),
        EnvWarning::EncryptedDecodeBlocked { source } => {
            format!("encrypted-decode-blocked: `{}` (intentional)", source)
        }
    }
}

/// Load the optional `.loctree/config.toml [env_truth]` config from a
/// snapshot root.
pub fn load_config_for(snapshot_root: &Path) -> Option<EnvTruthConfig> {
    precedence::load_config_override(snapshot_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fresh_repo() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn end_to_end_dotenv_only() {
        let tmp = fresh_repo();
        fs::write(
            tmp.path().join(".env"),
            "DATABASE_URL=postgres://localhost\n",
        )
        .unwrap();
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let report = compute_env_truth(&cfg, None);
        assert_eq!(report.declarations.len(), 1);
        assert_eq!(report.declarations[0].name, "DATABASE_URL");
        assert_eq!(report.declarations[0].sources.len(), 1);
    }

    #[test]
    fn orphan_code_reference_when_only_reads() {
        let tmp = fresh_repo();
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let mut snap = make_empty_snapshot();
        snap.semantic_facts = Some(SemanticFacts {
            env_contracts: vec![crate::semantic::EnvContract {
                name: "API_KEY".into(),
                used_in_files: vec!["src/lib.rs".into()],
                required_for: vec!["api auth".into()],
                occurrences: vec![],
            }],
            ..SemanticFacts::default()
        });
        let report = compute_env_truth(&cfg, Some(&snap));
        assert_eq!(report.orphan_reads.len(), 1);
        assert_eq!(report.orphan_reads[0].name, "API_KEY");
        let decl = report
            .declarations
            .iter()
            .find(|d| d.name == "API_KEY")
            .unwrap();
        assert!(
            decl.precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::OrphanCodeReference { .. }))
        );
    }

    #[test]
    fn precedence_sorted_descending() {
        let tmp = fresh_repo();
        fs::create_dir_all(tmp.path().join("k8s")).unwrap();
        fs::write(tmp.path().join(".env"), "SECRET=abc\n").unwrap();
        fs::write(
            tmp.path().join("k8s/sealed.yaml"),
            "
apiVersion: bitnami.com/v1alpha1
kind: SealedSecret
metadata:
  name: api
spec:
  encryptedData:
    SECRET: AgB...
",
        )
        .unwrap();
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let report = compute_env_truth(&cfg, None);
        let decl = report
            .declarations
            .iter()
            .find(|d| d.name == "SECRET")
            .unwrap();
        assert_eq!(decl.sources.len(), 2);
        // Highest-precedence (SealedSecret) should be first.
        assert!(decl.sources[0].precedence_rank > decl.sources[1].precedence_rank);
    }

    fn make_empty_snapshot() -> Snapshot {
        Snapshot {
            metadata: Default::default(),
            files: vec![],
            edges: vec![],
            export_index: Default::default(),
            command_bridges: vec![],
            event_bridges: vec![],
            barrels: vec![],
            semantic_facts: None,
            symbol_graph: None,
        }
    }

    /// Regression for the 2026-05-18 Screenscribe hak: `HEALTH_THRESHOLD`
    /// declared at workflow `env:` block was read three times in the same
    /// workflow's shell `run:` step (`$HEALTH_THRESHOLD`) yet env-truth
    /// reported `orphan-declaration: declared but never read`. Cross-block
    /// shell reads now merge into the read index so the warning stays
    /// silent for in-workflow self-references.
    #[test]
    fn workflow_shell_var_read_in_run_block_suppresses_orphan_declaration() {
        let tmp = fresh_repo();
        fs::create_dir_all(tmp.path().join(".github/workflows")).unwrap();
        fs::write(
            tmp.path().join(".github/workflows/ci.yml"),
            r#"name: CI
on: [push]
env:
  HEALTH_THRESHOLD: 50
jobs:
  gate:
    runs-on: ubuntu-latest
    steps:
      - name: check
        run: |
          HEALTH=99
          if [ "$HEALTH" -lt "$HEALTH_THRESHOLD" ]; then
            echo "::error::below threshold ($HEALTH_THRESHOLD)"
          else
            echo "Health score meets threshold ($HEALTH_THRESHOLD)"
          fi
"#,
        )
        .unwrap();
        // Provide a non-empty env_contracts so the orphan-declaration gate is open.
        let mut snap = make_empty_snapshot();
        snap.semantic_facts = Some(SemanticFacts {
            env_contracts: vec![crate::semantic::EnvContract {
                name: "UNRELATED_API_KEY".into(),
                used_in_files: vec!["src/lib.rs".into()],
                required_for: vec!["api auth".into()],
                occurrences: vec![],
            }],
            ..SemanticFacts::default()
        });
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let report = compute_env_truth(&cfg, Some(&snap));
        let decl = report
            .declarations
            .iter()
            .find(|d| d.name == "HEALTH_THRESHOLD")
            .unwrap_or_else(|| panic!("HEALTH_THRESHOLD declaration missing from report"));
        // Reads should now include the workflow self-reference.
        assert!(
            !decl.reads.is_empty(),
            "expected workflow shell `$HEALTH_THRESHOLD` reads to populate decl.reads"
        );
        // orphan-declaration must NOT fire when reads exist.
        assert!(
            !decl
                .precedence_warnings
                .iter()
                .any(|w| matches!(w, EnvWarning::OrphanDeclaration { .. })),
            "orphan-declaration warning fired despite cross-block shell reads"
        );
    }

    /// Regression for loctree-feedback entry 22: Makefile variables like
    /// `CURRENT_VERSION := ...` are make-local symbols, not runtime env reads.
    /// Old cached snapshots may still contain the previous
    /// `required_for = Makefile variable assignment` contract, so env-truth
    /// filters that namespace before it can become an orphan env read.
    #[test]
    fn makefile_variable_assignments_are_not_orphan_env_reads() {
        let tmp = fresh_repo();
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let mut snap = make_empty_snapshot();
        snap.semantic_facts = Some(SemanticFacts {
            env_contracts: vec![crate::semantic::EnvContract {
                name: "CURRENT_VERSION".into(),
                used_in_files: vec!["Makefile".into()],
                required_for: vec!["Makefile variable assignment".into()],
                occurrences: vec![],
            }],
            ..SemanticFacts::default()
        });

        let report = compute_env_truth(&cfg, Some(&snap));

        assert!(
            report
                .orphan_reads
                .iter()
                .all(|read| read.name != "CURRENT_VERSION"),
            "Makefile assignment leaked as orphan env read: {:?}",
            report.orphan_reads
        );
        assert!(
            report
                .declarations
                .iter()
                .all(|decl| decl.name != "CURRENT_VERSION"),
            "Makefile assignment leaked into env declarations: {:?}",
            report
                .declarations
                .iter()
                .map(|decl| decl.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// Regression for the 2026-05-18 Screenscribe hak: the markdown
    /// renderer used to print `plain '{value_hash}'` which operators
    /// read as the literal value. Now `value_hash` MUST be wrapped with
    /// an explicit `hash` / `sha256:` label so no one mistakes a hash
    /// for a value.
    #[test]
    fn render_value_presence_labels_hash_explicitly() {
        let rendered = render_value_presence(
            &ValuePresence::Plain {
                value_hash: "1a6562590ef1".into(),
            },
            true,
        );
        assert!(
            rendered.contains("hash"),
            "value_hash must be labeled `hash` to prevent operator confusion: got `{rendered}`"
        );
        assert!(
            rendered.contains("sha256:"),
            "value_hash should carry the `sha256:` prefix so the format is unambiguous: got `{rendered}`"
        );
    }

    /// W2-c: hashes hide behind `--hashes`; the default label is a bare
    /// `plain` so nobody mistakes 12 hex chars for the value.
    #[test]
    fn render_value_presence_hides_hash_by_default() {
        let rendered = render_value_presence(
            &ValuePresence::Plain {
                value_hash: "1a6562590ef1".into(),
            },
            false,
        );
        assert_eq!(rendered, "plain");
    }

    /// W2-c: templates never rank as live sources; their keys are compared
    /// in template-drift mode instead.
    #[test]
    fn template_files_excluded_from_ranking_and_drift_computed() {
        let tmp = fresh_repo();
        fs::write(tmp.path().join(".env"), "DATABASE_URL=live\nLIVE_ONLY=1\n").unwrap();
        fs::write(
            tmp.path().join(".env.example"),
            "DATABASE_URL=example\nTEMPLATE_ONLY_KEY=fill-me\n",
        )
        .unwrap();
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let report = compute_env_truth(&cfg, None);

        // No source anywhere points at the template.
        for decl in &report.declarations {
            assert!(
                decl.sources.iter().all(|s| !s.path.contains("example")),
                "template path leaked into sources for `{}`: {:?}",
                decl.name,
                decl.sources
            );
        }
        // TEMPLATE_ONLY_KEY has no live declaration at all.
        assert!(
            report
                .declarations
                .iter()
                .all(|d| d.name != "TEMPLATE_ONLY_KEY"),
            "template-only key must not become a declaration"
        );
        // Drift: missing-in-live = TEMPLATE_ONLY_KEY, extra-in-live = LIVE_ONLY.
        assert_eq!(report.template_drift.len(), 1);
        let drift = &report.template_drift[0];
        assert_eq!(drift.template_path, ".env.example");
        assert_eq!(drift.missing_in_live, vec!["TEMPLATE_ONLY_KEY".to_string()]);
        assert_eq!(drift.extra_in_live, vec!["LIVE_ONLY".to_string()]);
        assert_eq!(
            report.summary.warnings_by_kind.get("template_drift"),
            Some(&1)
        );
    }

    #[test]
    fn template_in_sync_produces_no_drift_entry() {
        let tmp = fresh_repo();
        fs::write(tmp.path().join(".env"), "DATABASE_URL=live\n").unwrap();
        fs::write(tmp.path().join(".env.example"), "DATABASE_URL=example\n").unwrap();
        let cfg = ComputeConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..ComputeConfig::default()
        };
        let report = compute_env_truth(&cfg, None);
        assert!(
            report.template_drift.is_empty(),
            "in-sync template must not produce drift: {:?}",
            report.template_drift
        );
    }
}
