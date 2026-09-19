import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "scripts" / "install-ai-hooks.sh"
HOOKS = ROOT / "ai-hooks"
GUARD = (
    HOOKS
    / "codex"
    / "loctree-marketplace"
    / "loctree-first"
    / "hooks"
    / "loctree-first-guard.py"
)


class LoctreeFirstGuardTests(unittest.TestCase):
    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.repo = self.root / "work"
        self.repo.mkdir()
        (self.repo / ".git").mkdir()
        (self.repo / "src").mkdir()
        (self.repo / "src" / "main.rs").write_text("fn main() {}\n")

    def tearDown(self):
        self.tempdir.cleanup()

    def run_guard(self, command):
        payload = {
            "session_id": "test-session",
            "cwd": str(self.repo),
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command},
        }
        # Supervisor PATH may bind `python3` to a stub that exits 127
        # unless VIBECRAFTED_PYTHON is set. Drive the guard with the same
        # interpreter as the delivery-verifier.
        return subprocess.run(
            [sys.executable, str(GUARD)],
            input=json.dumps(payload),
            text=True,
            capture_output=True,
            check=False,
        )

    def test_first_choice_rg_is_paused_and_teaches_the_contract(self):
        result = self.run_guard("rg -n main src")
        self.assertEqual(result.returncode, 2)
        self.assertIn("LOCTREE FIRST", result.stderr)
        self.assertIn("loct occurrences", result.stderr)
        self.assertIn("command rg", result.stderr)
        self.assertIn("loctree-fail.md", result.stderr)

    def test_deliberate_command_fallbacks_are_allowed(self):
        for command in ("command rg -n main src", "command grep -R main src"):
            with self.subTest(command=command):
                result = self.run_guard(command)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_pipe_filter_is_not_policed(self):
        result = self.run_guard("git status --short | grep '^ M'")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_w2_05_guard_split_respects_quotes(self):
        # [x] git commit -m "fix: grep && rg" is not blocked (quoted &&).
        quoted = self.run_guard('git commit -m "fix: grep && rg"')
        self.assertEqual(quoted.returncode, 0, quoted.stderr)

        # [x] A heredoc whose body contains the word grep is not blocked.
        heredoc = self.run_guard("cat <<'EOF'\ngrep -n main src/main.rs\nEOF")
        self.assertEqual(heredoc.returncode, 0, heredoc.stderr)

        escaped = self.run_guard("cat <<\\EOF\ngrep -n main src/main.rs\nEOF")
        self.assertEqual(escaped.returncode, 0, escaped.stderr)

        # [x] Real in-repo `grep -n` stays blocked; out-of-repo stays allowed.
        real = self.run_guard("grep -n main src/main.rs")
        self.assertEqual(real.returncode, 2, real.stderr)
        self.assertIn("LOCTREE FIRST", real.stderr)

        quoted_pattern = self.run_guard('grep -n "a && b" src/main.rs')
        self.assertEqual(quoted_pattern.returncode, 2, quoted_pattern.stderr)

        quoted_pipe = self.run_guard('grep -n "a | b" src/main.rs')
        self.assertEqual(quoted_pipe.returncode, 2, quoted_pipe.stderr)

        outside = self.root / "outside.txt"
        outside.write_text("needle\n")
        out_of_repo = self.run_guard(f"grep -n needle {outside}")
        self.assertEqual(out_of_repo.returncode, 0, out_of_repo.stderr)


class AiHooksInstallerTests(unittest.TestCase):
    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.home = Path(self.tempdir.name)
        self.bin = self.home / "bin"
        self.bin.mkdir()

    def tearDown(self):
        self.tempdir.cleanup()

    def fake_executable(self, name, body="exit 0\n"):
        path = self.bin / name
        path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body)
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    def run_installer(self, cli, hooks="all", extra_env=None):
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(self.home),
                "PATH": f"{self.bin}:{env['PATH']}",
                "CLI": cli,
                "HOOKS": hooks,
                "AI_HOOKS_SKIP_DOCTOR": "1",
            }
        )
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["bash", str(INSTALLER)],
            text=True,
            capture_output=True,
            check=False,
            env=env,
        )

    def test_claude_install_preserves_unrelated_hooks_and_replaces_memex(self):
        self.fake_executable("claude")
        settings = self.home / ".claude" / "settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(
            json.dumps(
                {
                    "hooks": {
                        "PreToolUse": [
                            {
                                "matcher": "Bash",
                                "hooks": [
                                    {"type": "command", "command": "keep-me"},
                                    {
                                        "type": "command",
                                        "command": "~/.claude/hooks/memex-context.sh",
                                    },
                                ],
                            }
                        ],
                        "SessionStart": [
                            {
                                "hooks": [
                                    {
                                        "type": "command",
                                        "command": "~/.claude/hooks/memex-startup.sh",
                                    }
                                ]
                            }
                        ],
                    }
                }
            )
        )
        legacy = self.home / ".claude" / "hooks"
        legacy.mkdir()
        for name in ("memex-context.sh", "memex-startup.sh", "memory-on-compact.sh"):
            (legacy / name).write_text("legacy\n")

        result = self.run_installer("claude")
        self.assertEqual(result.returncode, 0, result.stderr)

        installed = json.loads(settings.read_text())
        rendered = json.dumps(installed)
        self.assertIn("keep-me", rendered)
        self.assertNotIn("memex-context.sh", rendered)
        self.assertNotIn("memex-startup.sh", rendered)
        self.assertNotIn("memory-on-compact.sh", rendered)
        self.assertIn("loctree-first-guard.py", rendered)
        self.assertIn("aicx-precompact.sh", rendered)
        self.assertIn("aicx-postcompact.sh", rendered)
        for name in (
            "loctree-first-guard.py",
            "aicx-precompact.sh",
            "aicx-postcompact.sh",
            "aicx-recall-selftest.sh",
        ):
            self.assertTrue((legacy / name).is_file(), name)
        for name in ("memex-context.sh", "memex-startup.sh", "memory-on-compact.sh"):
            self.assertFalse((legacy / name).exists(), name)

    def test_codex_install_uses_versioned_marketplaces_and_plugins(self):
        calls = self.home / "codex-calls.log"
        self.fake_executable("codex", 'printf "%s\\n" "$*" >> "$CODEX_CALLS"\n')
        result = self.run_installer(
            "codex", extra_env={"CODEX_CALLS": str(calls)}
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        logged = calls.read_text()
        self.assertIn("plugin marketplace add", logged)
        self.assertIn("loctree-marketplace", logged)
        self.assertIn("aicx-marketplace", logged)
        self.assertIn("plugin add loctree-first@loctree-local", logged)
        self.assertIn("plugin add aicx-compact-recall@personal", logged)

    def test_codex_doctor_failure_aborts_the_install(self):
        self.fake_executable("codex")
        result = self.run_installer(
            "codex", hooks="aicx", extra_env={"AI_HOOKS_SKIP_DOCTOR": "0"}
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Codex AICX compact-recall doctor", result.stdout)

    def test_codex_update_preserves_cache_needed_by_running_processes(self):
        old_hook = (
            self.home
            / ".codex/plugins/cache/loctree-local/loctree-first/0.1.0/hooks/loctree-first-guard.py"
        )
        old_hook.parent.mkdir(parents=True)
        old_hook.write_text("old running hook\n")
        self.fake_executable(
            "codex",
            """
if [[ "$*" == *"plugin add loctree-first@loctree-local"* ]]; then
  rm -rf "$HOME/.codex/plugins/cache/loctree-local/loctree-first/0.1.0"
fi
""",
        )
        result = self.run_installer("codex", hooks="loctree")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(old_hook.is_file())
        self.assertEqual(old_hook.read_text(), "old running hook\n")

    def test_removed_memex_selector_fails_loudly(self):
        self.fake_executable("claude")
        result = self.run_installer("claude", hooks="memex")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("removed", (result.stdout + result.stderr).lower())

    def test_selective_install_preserves_the_other_live_package(self):
        self.fake_executable("claude")
        settings = self.home / ".claude" / "settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(
            json.dumps(
                {
                    "hooks": {
                        "PreToolUse": [
                            {
                                "matcher": "Bash",
                                "hooks": [
                                    {
                                        "type": "command",
                                        "command": "python3 ~/.claude/hooks/loctree-first-guard.py",
                                    }
                                ],
                            }
                        ],
                        "PreCompact": [
                            {
                                "hooks": [
                                    {
                                        "type": "command",
                                        "command": "bash ~/.claude/hooks/aicx-precompact.sh",
                                    }
                                ]
                            }
                        ],
                    }
                }
            )
        )

        result = self.run_installer("claude", hooks="loctree")
        self.assertEqual(result.returncode, 0, result.stderr)
        rendered = settings.read_text()
        self.assertIn("aicx-precompact.sh", rendered)

        result = self.run_installer("claude", hooks="aicx")
        self.assertEqual(result.returncode, 0, result.stderr)
        rendered = settings.read_text()
        self.assertIn("loctree-first-guard.py", rendered)

    def test_source_tree_contains_no_legacy_memex_payload(self):
        for name in ("memex-context.sh", "memex-startup.sh", "memory-on-compact.sh"):
            self.assertFalse((HOOKS / name).exists(), name)
        installer = INSTALLER.read_text()
        self.assertNotIn("INSTALL_MEMEX", installer)
        self.assertIn("doctor.sh", installer)


if __name__ == "__main__":
    unittest.main()
