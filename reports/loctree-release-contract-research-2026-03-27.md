# Loctree Suite Release Contract Research

Date: 2026-03-27

### Q1: What is the cleanest product/distribution model when one brand (`Loctree`) spans an OSS tier and a commercial tier, but the user-facing command should stay short (`loct`)?
**Sources**:  
`/home/polyversai/Libraxis/Loctree/Public/loctree/README.md`  
`/home/polyversai/Libraxis/Loctree/Public/loctree/loctree-rs/src/cli/entrypoint.rs`  
`/home/polyversai/Libraxis/Loctree/Public/loctree/distribution/npm/package.json`  
`/home/polyversai/Libraxis/Loctree/Suite/loctree-suite/distribution/npm/package.json`  
`/home/polyversai/Libraxis/Loctree/Suite/loctree-suite/distribution/homebrew/README.md`  
https://www.pulumi.com/pricing/  
https://docs.sentry.io/cli/installation/  

**Finding**:  
The cleanest contract is one product brand, one canonical command family, and tiering by edition/entitlement rather than by alternate executable names. For Loctree, that means:

- Brand: `Loctree`
- Canonical commands: `loct`, `loct-mcp`, `loct-lsp`
- Editions: `Loctree OSS` and `Loctree Pro` or `Loctree Suite`

Inference from the sources: the commercial line should not invent a second command identity. The active line should own the canonical `loct*` family, while `loctree` should be treated only as a compatibility bridge, not as a second active install contract.

**Confidence**: high

**Evidence**:  
- The public README already says `loct` is the canonical CLI command and `loctree` is a compatibility alias.  
- The public CLI entrypoint says both binaries share one implementation and `loctree` is the compatibility alias.  
- The public npm package still exports both `loctree` and `loct`, while the suite npm package exports only `loct`; that is evidence of an in-flight split, not a stable public contract.  
- Pulumi’s pricing page says the CLI/SDK are open source while Pulumi Cloud is the paid managed layer; same product brand, same CLI, different commercial tier.  
- Sentry’s install docs treat npm as a specialized distribution path for the same CLI rather than a second branded product.

### Q2: For a commercial active line fed from a private source repo, what are the practical implications of using `BSL-1.1` for source while distributing public binaries via GitHub Releases, npm, and Homebrew?
**Sources**:  
https://mariadb.com/bsl11/  
https://mariadb.com/bsl-faq-adopting/  
https://spdx.org/licenses/BUSL-1.1.html  
https://docs.npmjs.com/cli/v10/configuring-npm/package-json  
https://docs.brew.sh/Formula-Cookbook  
https://docs.brew.sh/Acceptable-Formulae  
`/home/polyversai/Libraxis/Loctree/Suite/loctree-suite/distribution/homebrew/README.md`  
`/home/polyversai/Libraxis/Loctree/Suite/loctree-suite/distribution/npm/package.json`  

**Finding**:  
`BUSL-1.1` is technically compatible with public binary distribution on GitHub Releases, npm, and a custom Homebrew tap, but it is a poor fit for a truly private-source active line.

Practical implications:

1. `BUSL-1.1` is not open source. MariaDB’s FAQ says that plainly.
2. It is an SPDX-recognized identifier, so npm and Homebrew metadata can represent it cleanly.
3. Homebrew core is off the table for a BUSL/private line, because Homebrew core requires open-source/DFSG licensing; Loctree must stay on its own tap.
4. If the active line’s source stays private, the market sees the restrictive side of BUSL but not the source-available upside that usually justifies BUSL in the first place.

Inference from the sources: if you want the commercial line to stay source-private, a conventional commercial license/EULA is cleaner than BUSL. If you want BUSL, the better product truth is to make the active line source-available and public enough for the BUSL promise to mean something.

**Confidence**: medium-high

**Evidence**:  
- MariaDB’s BUSL text grants rights to “copy, modify, create derivative works, redistribute, and make non-production use” and ties broader rights to the Change Date / Change License.  
- MariaDB’s BUSL FAQ says the BSL “is not an Open Source license”.  
- SPDX lists `BUSL-1.1`, which means package-manager metadata can name it directly.  
- npm docs say certain files are always included, including `package.json`, `README`, and `LICENSE / LICENCE`; that supports shipping a public npm binary wrapper with a BUSL license file.  
- Homebrew’s Formula Cookbook uses SPDX license expressions.  
- Homebrew’s Acceptable Formulae policy says core formulae “must be open-source”, so a BUSL line belongs in Loctree’s own tap, not `homebrew/core`.

### Q3: What are the best-practice ways to avoid or document package-manager collisions when the same CLI binary name may be installed via both Homebrew and npm?
**Sources**:  
https://docs.npmjs.com/cli/v10/configuring-npm/folders  
https://docs.npmjs.com/cli/v10/configuring-npm/package-json  
https://docs.brew.sh/Common-Issues  
https://docs.brew.sh/Formula-Cookbook  
https://docs.sentry.io/cli/installation/  
`/home/polyversai/Libraxis/Loctree/Public/loctree/distribution/npm/README.md`  
`/home/polyversai/Libraxis/Loctree/Suite/loctree-suite/distribution/npm/README.md`  
Local experiment on 2026-03-27 with `npm install -g --prefix <tmp> --ignore-scripts` against both Loctree npm packages

**Finding**:  
Package-manager collisions are not something to finesse away with documentation alone. They are the expected result when two installers both try to own the same PATH-level executable. The best practice is to define channel ownership:

- one canonical machine-global installer per OS
- npm used locally (`npx`, devDependency, CI/build servers) unless it is the only practical system channel
- explicit migration/uninstall guidance when switching channels

For Loctree specifically:

- Homebrew or the first-party installer should own machine-global `loct` on macOS/Linux.
- npm should be documented primarily for local/project use and CI.
- only one active npm package should own `loct`
- the legacy `loctree` npm surface should stop exporting a second active global `loct` contract

**Confidence**: high

**Evidence**:  
- npm docs say executables are linked into `{prefix}/bin` on Unix.  
- Homebrew docs show the normal failure mode: “Could not symlink ... Target ... already exists.”  
- Formula Cookbook says `conflicts_with` is a last resort and also notes formula authors can rename installed binaries to avoid collisions, but that would weaken Loctree’s desired short command contract.  
- Sentry’s official docs say npm installation is for “specialized use cases” such as build servers, and global npm installation “is not recommended”. That is a strong precedent for separating local/build tooling from machine-global ownership.  
- Loctree’s public npm README already warns users not to mix Homebrew and `npm -g` for the same machine-level CLI.  
- Local experiment: if a preexisting `loct` or `loctree` binary is already present in the target prefix, npm exits with `EEXIST` and tells the user to remove the file or use `--force`.

### Q4: What do strong local-first CLI tools do for cache growth control: default size caps, age-based retention, project-count limits, or explicit manual cleaning only?
**Sources**:  
`/home/polyversai/Libraxis/Loctree/Public/loctree/loctree-rs/src/cli/dispatch/handlers/cache.rs`  
`/home/polyversai/Libraxis/Loctree/Public/loctree/loctree-rs/src/cli/command/help_texts.rs`  
https://docs.astral.sh/uv/concepts/cache/  
https://pnpm.io/cli/store  
https://ccache.dev/manual/latest.html  

**Finding**:  
Strong local-first tooling usually does not stop at manual cleaning only.

Observed patterns from the sources:

- `uv`: explicit prune commands; safe periodic cleanup; CI-specific prune mode
- `pnpm`: explicit prune of unreferenced packages; recommended occasionally, not constantly
- `ccache`: automatic cleanup when `max_size` or `max_files` is exceeded, with approximate LRU behavior; also supports manual cleanup and age-based eviction

Loctree today is closer to “manual cleanup with optional age filter” than to a mature artifact-cache policy. That is too weak for a cache that can quietly grow to tens of gigabytes across thousands of projects.

Inference from the sources: Loctree should move to a hybrid policy, not manual-only:

- automatic size cap
- age-based retention
- project-count cap
- explicit manual cleaning still available

**Confidence**: high

**Evidence**:  
- Loctree’s current help text exposes `loct cache list` and `loct cache clean`, with `--project` and `--older-than 30d`; there is no automatic retention policy in the handler.  
- `uv` docs say `uv cache prune` removes unused entries and is “safe to run periodically”.  
- `pnpm` docs say `pnpm store prune` removes unreferenced packages and is best run “occasionally”.  
- `ccache` says it triggers “automatic cleanup” when `max_size` or `max_files` is exceeded, removes entries in approximate LRU order, and also supports `--evict-older-than`.

### Synthesis
- Recommended approach: keep `Loctree` as the umbrella product brand, make `loct` the only canonical CLI contract, treat OSS vs Pro as editions, and stop shipping two active package-manager stories that both claim the same global binary.
- Alternatives considered: separate command names per tier (`loct` vs `loct-pro`) would reduce licensing ambiguity but would fracture docs, UX, and muscle memory; keeping both `loctree` and `loct` as active npm/global surfaces preserves short-term continuity but hardens install confusion and collision support load; using BUSL for a private-source line is possible in metadata terms, but weaker as a market story than either a true source-available line or a straightforward commercial license.
- Open questions: whether the active commercial line is intended to be source-available or truly source-private; what the free Additional Use Grant should be, if any; whether Windows gets a first-party installer/channel besides npm; when the crate rename track lands for Cargo surfaces.
- Implementation notes: freeze the public `loctree` npm package as a compatibility line rather than a release target; keep official Homebrew delivery in Loctree-owned taps; make npm docs push `npx loct` and project-local install first; add install preflight checks for existing `loct` ownership; reuse Loctree’s existing cache `--older-than` and size enumeration logic to implement automatic garbage collection after writes.

- recommended release contract: `Loctree` is the brand, `loct`/`loct-mcp`/`loct-lsp` are the canonical commands, OSS and Pro are edition labels, and `loctree` remains only as a transitional compatibility alias instead of a second active distribution truth.
- recommended license posture: keep the public OSS line on `MIT OR Apache-2.0`; for the commercial active line, use `BUSL-1.1` only if the line is genuinely source-available, otherwise prefer a conventional commercial proprietary license over a private-source BUSL story.
- recommended install-channel policy: first-party installer and Homebrew tap own machine-global installs on supported Unix platforms; npm is primarily for local/project use and CI; only one active npm package may own `loct`; the legacy `loctree` npm package should be deprecated or reduced to a non-global compatibility shim.
- recommended cache-retention baseline: automatic GC enabled by default with approximate LRU eviction, total cache cap of 10 GB, max 500 cached projects, stale-project eviction after 30 days, and manual commands (`list`, `clean`, `clean --project`, `clean --older-than`) retained as explicit escape hatches.
