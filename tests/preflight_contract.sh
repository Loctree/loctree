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

# Preserve the proven npm trusted-publisher identity, with manual-only hosted
# execution and signed canonical assets. The suite remains the binary authority.
workflow="$ROOT_DIR/.github/workflows/publish.yml"
for expected in 'workflow_dispatch:' 'id-token: write' 'environment: npm-publish' 'runs-on: ubuntu-latest' 'git verify-tag' 'gpg --batch --verify' 'AUTH_MODE: oidc'; do
  if ! grep -Fq "$expected" "$workflow"; then
    echo "trusted npm publisher is missing: $expected" >&2
    exit 1
  fi
done
if grep -Eq '^  (push|pull_request|release):|self-hosted|secrets.NPM_TOKEN|cargo build' "$workflow"; then
  echo "npm publisher must remain manual, hosted, token-free and consume released binaries" >&2
  exit 1
fi
if [ -e "$ROOT_DIR/.github/workflows/release-bundles.yml" ]; then
  echo "binary publication remains owned by the canonical suite" >&2
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
codeql_java_setup_step="$(awk '
  /- name: Set up Java for JetBrains plugin analysis/ { flag=1; print; next }
  flag && /^      - name: / { exit }
  flag { print }
' "$CODEQL_WORKFLOW")"
codeql_java_build_step="$(awk '
  /- name: Build JetBrains plugin for CodeQL/ { flag=1; print; next }
  flag && /^      - name: / { exit }
  flag { print }
' "$CODEQL_WORKFLOW")"
codeql_java_matrix_entry="$(awk '
  /- language: java-kotlin/ { flag=1; print; next }
  flag && /^          - language: / { exit }
  flag && /^    steps:/ { exit }
  flag { print }
' "$CODEQL_WORKFLOW")"
if ! grep -Fq 'name: Analyze (${{ matrix.language }})' "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not keep per-language analyze jobs" >&2
  exit 1
fi
if ! grep -Fq "language: java-kotlin" "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not analyze the JetBrains Kotlin surface" >&2
  exit 1
fi
if ! printf '%s\n' "$codeql_java_matrix_entry" | grep -Fq "build-mode: manual"; then
  echo "CodeQL workflow does not opt Java/Kotlin into an explicit build" >&2
  exit 1
fi
if ! printf '%s\n' "$codeql_java_setup_step" | grep -Fq "name: Set up Java for JetBrains plugin analysis"; then
  echo "CodeQL workflow does not declare a dedicated Java setup step for the Kotlin lane" >&2
  exit 1
fi
if ! printf '%s\n' "$codeql_java_setup_step" | grep -Fq "distribution: temurin"; then
  echo "CodeQL workflow does not request the Temurin Java distribution for the Kotlin lane" >&2
  exit 1
fi
if ! printf '%s\n' "$codeql_java_build_step" | grep -Fq "working-directory: editors/jetbrains"; then
  echo "CodeQL workflow does not build the JetBrains Gradle project in place" >&2
  exit 1
fi
if ! printf '%s\n' "$codeql_java_build_step" | grep -Fq "./gradlew --no-daemon classes testClasses"; then
  echo "CodeQL workflow does not compile the JetBrains Gradle sources before analysis" >&2
  exit 1
fi
if ! grep -Fq 'category: "/language:${{ matrix.language }}"' "$CODEQL_WORKFLOW"; then
  echo "CodeQL workflow does not separate per-language SARIF categories" >&2
  exit 1
fi
