#!/bin/sh
# Literal Make expressions below are contract strings, not shell expansions.
# shellcheck disable=SC2016
set -eu

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
bash "$ROOT_DIR/tests/crates_io_index_wait_contract.sh"
dry_run="$(make -sn -C "$ROOT_DIR" preflight)"
test_dry_run="$(make -sn -C "$ROOT_DIR" test | tail -n 1)"

if ! grep -Fq 'preflight: test-git-hooks' "$ROOT_DIR/Makefile"; then
  echo "preflight does not include the Git hook and isolation regressions" >&2
  exit 1
fi

if ! grep -Fq 'LOCTREE_GIT_LOCAL_ENV_VARS := $(shell git rev-parse --local-env-vars 2>/dev/null)' "$ROOT_DIR/Makefile"; then
  echo "Makefile does not discover Git's repository-local environment" >&2
  exit 1
fi

if ! grep -Fq 'unexport $(LOCTREE_GIT_LOCAL_ENV_VARS)' "$ROOT_DIR/Makefile"; then
  echo "Makefile does not isolate all recipe prerequisites from repository-local Git environment" >&2
  exit 1
fi

if grep -Fq 'PROTOC_VENDOR :=' "$ROOT_DIR/Makefile"; then
  echo "Makefile still runs the unused parse-time protoc discovery" >&2
  exit 1
fi

if ! printf '%s\n' "$dry_run" | grep -Fq 'sh tools/preflight.sh'; then
  echo "preflight target does not invoke tools/preflight.sh" >&2
  exit 1
fi

install_recipe="$(sed -n '/^install:/,/^install-all:/p' "$ROOT_DIR/Makefile")"
install_all_recipe="$(sed -n '/^install-all:/,/^setup-protoc:/p' "$ROOT_DIR/Makefile")"
if printf '%s\n%s\n' "$install_recipe" "$install_all_recipe" | grep -Fq 'git-hooks'; then
  echo "binary installation must not change the repository hook policy" >&2
  exit 1
fi

if [ "$test_dry_run" != "sh tools/test.sh" ]; then
  echo "test target does not invoke tools/test.sh" >&2
  exit 1
fi

if ! grep -Fq 'sh tools/test.sh || exit 1;' "$ROOT_DIR/Makefile"; then
  echo "publish target does not invoke the isolated test wrapper" >&2
  exit 1
fi

RELEASE_BUNDLES_WORKFLOW="$ROOT_DIR/.github/workflows/release-bundles.yml"
if grep -Eq 'tar -tzf .*\|.*grep -[A-Za-z]*q' "$RELEASE_BUNDLES_WORKFLOW"; then
  echo "release bundle membership check is vulnerable to pipefail/SIGPIPE false negatives" >&2
  exit 1
fi
if [ "$(grep -Fc "format('refs/tags/v{0}', inputs.version)" "$RELEASE_BUNDLES_WORKFLOW")" -ne 2 ]; then
  echo "manual release bundle recovery does not pin both checkouts to the requested tag" >&2
  exit 1
fi
if ! grep -Fq 'git describe --tags --exact-match HEAD' "$RELEASE_BUNDLES_WORKFLOW"; then
  echo "release bundle verification does not prove the checkout is the exact requested tag" >&2
  exit 1
fi
if ! grep -Fq "|| github.event_name == 'workflow_dispatch'" "$RELEASE_BUNDLES_WORKFLOW"; then
  echo "manual release bundle recovery cannot reach the public release publish job" >&2
  exit 1
fi

PUBLISH_WORKFLOW="$ROOT_DIR/.github/workflows/publish.yml"
publish_npm_job="$(sed -n '/^  publish-npm:/,/^  publish-monorepo:/p' "$PUBLISH_WORKFLOW")"
if ! printf '%s\n' "$publish_npm_job" | grep -Fq 'node-version: "24"'; then
  echo "npm publish job does not pin a current Node runtime" >&2
  exit 1
fi
if ! printf '%s\n' "$publish_npm_job" | grep -Fq 'npm install -g npm@11.17.0'; then
  echo "npm publish job does not pin the tested trusted-publishing client" >&2
  exit 1
fi
if printf '%s\n' "$publish_npm_job" | grep -Fq 'npm install -g npm@latest'; then
  echo "npm publish job still follows the moving npm latest tag" >&2
  exit 1
fi

for expected in \
  'cargo fmt --all -- --check' \
  'cargo clippy --workspace --all-targets -- -D warnings' \
  'cargo check --workspace' \
  'cargo test --workspace' \
  'cargo build -p loctree --release --quiet' \
  'target/release/loct'
do
  if ! grep -Fq "$expected" "$ROOT_DIR/tools/preflight.sh"; then
    echo "preflight is missing: $expected" >&2
    exit 1
  fi
done

for script in tools/preflight.sh tools/test.sh; do
  if ! grep -Fq 'loctree_clear_local_git_env' "$ROOT_DIR/$script"; then
    echo "$script does not isolate repository-local Git environment" >&2
    exit 1
  fi
done

CI_WORKFLOW="$ROOT_DIR/.github/workflows/ci.yml"
ci_executable="$(sed 's/[[:space:]]*#.*$//' "$CI_WORKFLOW")"
if ! printf '%s\n' "$ci_executable" | grep -Fq 'pull_request: {}'; then
  echo "CI does not cover internal pull requests to stacked base branches" >&2
  exit 1
fi
# Accept both spellings of the label. It used to be a JSON literal
# (`runner: '"ubuntu-latest"'`) because the same matrix key also had to carry
# an array of self-hosted labels through `fromJSON`. With self-hosted retired
# the value is a plain scalar, and this contract tests the routing, not the
# quoting that a since-removed indirection happened to require.
if ! printf '%s\n' "$ci_executable" | grep -Eq "runner: '?\"?ubuntu-latest\"?'?[[:space:]]*\$"; then
  echo "authoritative Linux CI is not routed to a hosted runner" >&2
  exit 1
fi
# Stronger than the old ops-linux-only check: no self-hosted host may be
# trusted with repository code, whatever it is named. Comments are stripped
# from $ci_executable above, so historical mentions do not trip this.
if printf '%s\n' "$ci_executable" | grep -Eq 'self-hosted|ops-linux|dragon-macos'; then
  echo "CI still schedules work on a self-hosted runner" >&2
  exit 1
fi
if ! printf '%s\n' "$ci_executable" | grep -Eq \
  "^[[:space:]]+if:[[:space:]]+github.event_name != 'pull_request' \\|\\| github.event.pull_request.head.repo.full_name == github.repository[[:space:]]*$"; then
  echo "self-hosted CI is not guarded from fork pull requests" >&2
  exit 1
fi

CODEQL_WORKFLOW="$ROOT_DIR/.github/workflows/codeql.yml"
if ! grep -Fq 'name: Analyze (${{ matrix.language }})' "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not keep per-language analyze jobs" >&2
  exit 1
fi
if ! grep -Fq "language: java-kotlin" "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not analyze the JetBrains Kotlin surface" >&2
  exit 1
fi
if ! grep -Fq "build-mode: manual" "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not opt Java/Kotlin into an explicit build" >&2
  exit 1
fi
if ! grep -Fq "uses: actions/setup-java@v4" "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not set up Java deterministically for the Kotlin lane" >&2
  exit 1
fi
if ! grep -Fq 'java-version: "21"' "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not pin the required Java toolchain for the Kotlin lane" >&2
  exit 1
fi
if ! grep -Fq "working-directory: editors/jetbrains" "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not build the JetBrains Gradle project in place" >&2
  exit 1
fi
if ! grep -Fq "./gradlew --no-daemon classes testClasses" "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not compile the JetBrains Gradle sources before analysis" >&2
  exit 1
fi
if ! grep -Fq 'category: "/language:${{ matrix.language }}"' "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not separate per-language SARIF categories" >&2
  exit 1
fi
