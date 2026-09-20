#!/bin/sh
set -eu

ROOT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/loctree-git-hooks.XXXXXX")"
MAIN_REPO="$TMP_ROOT/main"
WORKTREE="$TMP_ROOT/worktree"
REMOTE_REPO="$TMP_ROOT/remote.git"
HOOK_SENTINEL="$TMP_ROOT/stale-pre-push-ran"

cleanup() {
  rm -R "$TMP_ROOT"
}
trap cleanup EXIT

git init -q "$MAIN_REPO"
git -C "$MAIN_REPO" config user.name "Loctree Test"
git -C "$MAIN_REPO" config user.email "loctree-test@example.invalid"
printf '%s\n' "fixture" > "$MAIN_REPO/README.md"
git -C "$MAIN_REPO" add README.md
git -C "$MAIN_REPO" commit --no-verify -qm "test: seed worktree fixture"
git -C "$MAIN_REPO" worktree add -q -b hook-test "$WORKTREE"

# The main checkout deliberately represents an older branch. Its tracked
# pre-push hook would execute if core.hooksPath still resolved per checkout.
mkdir -p "$MAIN_REPO/tools/hooks"
cat > "$MAIN_REPO/tools/hooks/pre-push" <<EOF
#!/bin/sh
printf '%s\n' stale > "$HOOK_SENTINEL"
exit 97
EOF
chmod +x "$MAIN_REPO/tools/hooks/pre-push"
git -C "$MAIN_REPO" config --local core.hooksPath tools/hooks

# The current worktree installs an immutable snapshot outside every checkout.
mkdir -p "$WORKTREE/tools"
cp -R "$ROOT_DIR/tools/hooks" "$WORKTREE/tools/hooks"
cp "$ROOT_DIR/tools/install-git-hooks.sh" "$WORKTREE/tools/install-git-hooks.sh"
chmod +x "$WORKTREE/tools/install-git-hooks.sh"
git -C "$WORKTREE" add tools
git -C "$WORKTREE" commit --no-verify -qm "fix(hooks): install safe hook source"
make -s -C "$WORKTREE" -f "$ROOT_DIR/Makefile" git-hooks

common_dir="$(git -C "$WORKTREE" rev-parse --git-common-dir)"
case "$common_dir" in
  /*) ;;
  *) common_dir="$WORKTREE/$common_dir" ;;
esac
common_dir="$(CDPATH='' cd -- "$common_dir" && pwd -P)"
source_revision="$(git -C "$WORKTREE" rev-parse HEAD)"
expected_hooks_path="$common_dir/loctree-hooks/$source_revision"
hooks_path="$(git -C "$WORKTREE" config --get core.hooksPath)"

if [ "$hooks_path" != "$expected_hooks_path" ]; then
  echo "expected absolute common hooks path $expected_hooks_path, got: $hooks_path" >&2
  exit 1
fi
if [ "$(git -C "$MAIN_REPO" config --get core.hooksPath)" != "$expected_hooks_path" ]; then
  echo "linked worktrees do not share the installed hook snapshot" >&2
  exit 1
fi
if [ "$(git -C "$MAIN_REPO" config --worktree --get core.hooksPath)" != "$expected_hooks_path" ] ||
   [ "$(git -C "$WORKTREE" config --worktree --get core.hooksPath)" != "$expected_hooks_path" ]; then
  echo "existing worktrees do not pin the installed hook snapshot" >&2
  exit 1
fi

for hook in pre-commit commit-msg lib/commit-msg-diff-gate.sh; do
  if [ ! -x "$expected_hooks_path/$hook" ]; then
    echo "expected executable installed hook: $hook" >&2
    exit 1
  fi
done
if [ -e "$expected_hooks_path/pre-push" ]; then
  echo "installed hook snapshot must not contain pre-push" >&2
  exit 1
fi

git -C "$MAIN_REPO" hook run pre-commit
git -C "$WORKTREE" hook run pre-commit

# Reproduce both historical installers after migration: one rewrites the common
# local config, the other recreates a default-dir symlink. Neither may override
# the per-worktree immutable snapshot.
git -C "$MAIN_REPO" config --local core.hooksPath tools/hooks
ln -sf ../../tools/hooks/pre-push "$common_dir/hooks/pre-push"
chmod +x "$MAIN_REPO/tools/hooks/pre-push"
if [ "$(git -C "$MAIN_REPO" config --get core.hooksPath)" != "$expected_hooks_path" ] ||
   [ "$(git -C "$WORKTREE" config --get core.hooksPath)" != "$expected_hooks_path" ]; then
  echo "legacy local config write overrode a worktree snapshot" >&2
  exit 1
fi

# A real push path from the deliberately stale checkout must not execute the
# recreated symlink or its branch-controlled tools/hooks/pre-push.
git init -q --bare "$REMOTE_REPO"
git -C "$MAIN_REPO" remote add fixture "$REMOTE_REPO"
git -C "$MAIN_REPO" push --dry-run fixture HEAD:refs/heads/main >/dev/null
if [ -e "$HOOK_SENTINEL" ]; then
  echo "stale worktree pre-push hook executed" >&2
  exit 1
fi

# Reinstallation repairs the shared fallback without changing the immutable
# generation used by existing worktrees.
make -s -C "$WORKTREE" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null
if [ "$(git -C "$WORKTREE" config --local --get core.hooksPath)" != "$expected_hooks_path" ]; then
  echo "reinstallation did not repair the shared hook fallback" >&2
  exit 1
fi

# A managed generation is immutable: added hooks and mode tampering both fail
# closed instead of activating modified code.
printf '%s\n' '#!/bin/sh' 'exit 0' > "$expected_hooks_path/pre-push"
chmod +x "$expected_hooks_path/pre-push"
if make -s -C "$WORKTREE" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject an added managed pre-push hook" >&2
  exit 1
fi
rm -f "$expected_hooks_path/pre-push"
chmod -x "$expected_hooks_path/pre-commit"
if make -s -C "$WORKTREE" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject a non-executable managed hook" >&2
  exit 1
fi
chmod +x "$expected_hooks_path/pre-commit"

# Tracked and clean is not sufficient source trust: symlinks can redirect
# execution, while a missing executable bit is a reviewed-contract drift.
SOURCE_POLICY_REPO="$TMP_ROOT/source-policy"
EXTERNAL_HOOK="$TMP_ROOT/external-pre-commit"
git init -q "$SOURCE_POLICY_REPO"
git -C "$SOURCE_POLICY_REPO" config user.name "Loctree Test"
git -C "$SOURCE_POLICY_REPO" config user.email "loctree-test@example.invalid"
mkdir -p "$SOURCE_POLICY_REPO/tools"
cp -R "$ROOT_DIR/tools/hooks" "$SOURCE_POLICY_REPO/tools/hooks"
cp "$ROOT_DIR/tools/install-git-hooks.sh" \
  "$SOURCE_POLICY_REPO/tools/install-git-hooks.sh"
chmod +x "$SOURCE_POLICY_REPO/tools/install-git-hooks.sh"
git -C "$SOURCE_POLICY_REPO" add tools
git -C "$SOURCE_POLICY_REPO" commit --no-verify -qm "test: seed hook source policy fixture"

printf '%s\n' '#!/bin/sh' 'exit 0' > "$EXTERNAL_HOOK"
chmod +x "$EXTERNAL_HOOK"
rm "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
ln -s "$EXTERNAL_HOOK" "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
git -C "$SOURCE_POLICY_REPO" add tools/hooks/pre-commit
git -C "$SOURCE_POLICY_REPO" commit --no-verify -qm "test: track symlinked hook source"
if make -s -C "$SOURCE_POLICY_REPO" -f "$ROOT_DIR/Makefile" \
  git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject a symlinked hook source" >&2
  exit 1
fi
if git -C "$SOURCE_POLICY_REPO" config --get core.hooksPath >/dev/null 2>&1; then
  echo "hooksPath changed after rejecting a symlinked hook source" >&2
  exit 1
fi

rm "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
cp "$ROOT_DIR/tools/hooks/pre-commit" \
  "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
chmod -x "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
git -C "$SOURCE_POLICY_REPO" add tools/hooks/pre-commit
git -C "$SOURCE_POLICY_REPO" commit --no-verify -qm "test: track non-executable hook source"
if make -s -C "$SOURCE_POLICY_REPO" -f "$ROOT_DIR/Makefile" \
  git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject a non-executable hook source" >&2
  exit 1
fi
if git -C "$SOURCE_POLICY_REPO" config --get core.hooksPath >/dev/null 2>&1; then
  echo "hooksPath changed after rejecting a non-executable hook source" >&2
  exit 1
fi

# Index hints must not hide unreviewed working-tree bytes inside a snapshot
# whose directory name claims the current HEAD revision.
cp "$ROOT_DIR/tools/hooks/pre-commit" \
  "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
chmod +x "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
git -C "$SOURCE_POLICY_REPO" add tools/hooks/pre-commit
git -C "$SOURCE_POLICY_REPO" commit --no-verify -qm "test: restore executable hook source"
git -C "$SOURCE_POLICY_REPO" update-index --assume-unchanged \
  tools/hooks/pre-commit
printf '%s\n' '#!/bin/sh' 'exit 42' > \
  "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
chmod +x "$SOURCE_POLICY_REPO/tools/hooks/pre-commit"
if ! git -C "$SOURCE_POLICY_REPO" diff --quiet HEAD -- tools/hooks/pre-commit; then
  echo "assume-unchanged fixture unexpectedly appears dirty" >&2
  exit 1
fi
if make -s -C "$SOURCE_POLICY_REPO" -f "$ROOT_DIR/Makefile" \
  git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject assume-unchanged hook bytes" >&2
  exit 1
fi
if git -C "$SOURCE_POLICY_REPO" config --get core.hooksPath >/dev/null 2>&1; then
  echo "hooksPath changed after rejecting assume-unchanged hook bytes" >&2
  exit 1
fi

# Installing from source must not silently replace a foreign hook policy.
CUSTOM_REPO="$TMP_ROOT/custom"
git init -q "$CUSTOM_REPO"
mkdir -p "$CUSTOM_REPO/custom-hooks" "$CUSTOM_REPO/tools"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$CUSTOM_REPO/custom-hooks/pre-commit"
chmod +x "$CUSTOM_REPO/custom-hooks/pre-commit"
git -C "$CUSTOM_REPO" config --local core.hooksPath custom-hooks
cp -R "$ROOT_DIR/tools/hooks" "$CUSTOM_REPO/tools/hooks"
cp "$ROOT_DIR/tools/install-git-hooks.sh" "$CUSTOM_REPO/tools/install-git-hooks.sh"
chmod +x "$CUSTOM_REPO/tools/install-git-hooks.sh"

if make -s -C "$CUSTOM_REPO" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject a foreign core.hooksPath" >&2
  exit 1
fi
if [ "$(git -C "$CUSTOM_REPO" config --get core.hooksPath)" != "custom-hooks" ]; then
  echo "foreign core.hooksPath changed after rejected installation" >&2
  exit 1
fi

# The historical path name is not sufficient ownership proof when an
# additional executable hook is present.
git -C "$CUSTOM_REPO" config --local core.hooksPath tools/hooks
printf '%s\n' '#!/bin/sh' 'exit 0' > "$CUSTOM_REPO/tools/hooks/post-checkout"
chmod +x "$CUSTOM_REPO/tools/hooks/post-checkout"
if make -s -C "$CUSTOM_REPO" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject an additional tracked hook" >&2
  exit 1
fi
if [ "$(git -C "$CUSTOM_REPO" config --get core.hooksPath)" != "tools/hooks" ]; then
  echo "tracked hook policy changed after rejected installation" >&2
  exit 1
fi

# The default .git/hooks directory is also policy when core.hooksPath is unset.
# A foreign executable there must remain active and block migration.
DEFAULT_REPO="$TMP_ROOT/default"
git init -q "$DEFAULT_REPO"
mkdir -p "$DEFAULT_REPO/tools"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$DEFAULT_REPO/.git/hooks/pre-commit"
chmod +x "$DEFAULT_REPO/.git/hooks/pre-commit"
cp -R "$ROOT_DIR/tools/hooks" "$DEFAULT_REPO/tools/hooks"
cp "$ROOT_DIR/tools/install-git-hooks.sh" "$DEFAULT_REPO/tools/install-git-hooks.sh"
chmod +x "$DEFAULT_REPO/tools/install-git-hooks.sh"

if make -s -C "$DEFAULT_REPO" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject a foreign default hook" >&2
  exit 1
fi
if git -C "$DEFAULT_REPO" config --get core.hooksPath >/dev/null 2>&1; then
  echo "core.hooksPath was set after rejecting a foreign default hook" >&2
  exit 1
fi
if [ ! -x "$DEFAULT_REPO/.git/hooks/pre-commit" ]; then
  echo "foreign default hook changed after rejected installation" >&2
  exit 1
fi

# A global/system-style policy must not be shadowed by a new local override.
GLOBAL_REPO="$TMP_ROOT/global"
GLOBAL_CONFIG="$TMP_ROOT/global.gitconfig"
git init -q "$GLOBAL_REPO"
mkdir -p "$GLOBAL_REPO/tools"
cp -R "$ROOT_DIR/tools/hooks" "$GLOBAL_REPO/tools/hooks"
cp "$ROOT_DIR/tools/install-git-hooks.sh" "$GLOBAL_REPO/tools/install-git-hooks.sh"
chmod +x "$GLOBAL_REPO/tools/install-git-hooks.sh"
git config --file "$GLOBAL_CONFIG" core.hooksPath global-hooks
if GIT_CONFIG_GLOBAL="$GLOBAL_CONFIG" \
  make -s -C "$GLOBAL_REPO" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject an effective global hooksPath" >&2
  exit 1
fi
if git -C "$GLOBAL_REPO" config --local --get core.hooksPath >/dev/null 2>&1; then
  echo "local hooksPath shadowed a global policy after rejected installation" >&2
  exit 1
fi

# Enabling worktreeConfig must not awaken a dormant foreign worktree policy.
DORMANT_REPO="$TMP_ROOT/dormant"
DORMANT_WORKTREE="$TMP_ROOT/dormant-worktree"
git init -q "$DORMANT_REPO"
git -C "$DORMANT_REPO" config user.name "Loctree Test"
git -C "$DORMANT_REPO" config user.email "loctree-test@example.invalid"
mkdir -p "$DORMANT_REPO/tools"
cp -R "$ROOT_DIR/tools/hooks" "$DORMANT_REPO/tools/hooks"
cp "$ROOT_DIR/tools/install-git-hooks.sh" "$DORMANT_REPO/tools/install-git-hooks.sh"
chmod +x "$DORMANT_REPO/tools/install-git-hooks.sh"
git -C "$DORMANT_REPO" add tools
git -C "$DORMANT_REPO" commit --no-verify -qm "fix(hooks): seed dormant policy fixture"
git -C "$DORMANT_REPO" worktree add -q -b dormant-test "$DORMANT_WORKTREE"
dormant_git_dir="$(git -C "$DORMANT_WORKTREE" rev-parse --git-dir)"
git config --file "$dormant_git_dir/config.worktree" core.hooksPath foreign-hooks
if make -s -C "$DORMANT_REPO" -f "$ROOT_DIR/Makefile" git-hooks >/dev/null 2>&1; then
  echo "expected git-hooks to reject a dormant foreign worktree policy" >&2
  exit 1
fi
if git -C "$DORMANT_REPO" config --get extensions.worktreeConfig >/dev/null 2>&1; then
  echo "worktreeConfig was enabled after rejecting a dormant foreign policy" >&2
  exit 1
fi
if [ "$(git config --file "$dormant_git_dir/config.worktree" --get core.hooksPath)" != "foreign-hooks" ]; then
  echo "dormant foreign worktree policy changed after rejection" >&2
  exit 1
fi

printf '%s\n' 'git hook installation: ok'
