from pathlib import Path


PLUGIN_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = PLUGIN_ROOT.parent


def test_main_skill_separates_literal_truth_from_discovery() -> None:
    text = (PLUGIN_ROOT / "skills/loctree/SKILL.md").read_text(encoding="utf-8")
    assert "Plain `loct find QUERY` is exact identifier-boundary" in text
    assert "`--discover`" in text
    assert "zero-consumer or dead-code result is a candidate" in text
    assert "Replace grep entirely" not in text
    assert "PostToolUse" not in text


def test_find_skill_carries_real_regression_examples() -> None:
    text = (PLUGIN_ROOT / "skills/loctree-find/SKILL.md").read_text(encoding="utf-8")
    assert "LOCT_OPEN_BROWSER_ENV" in text
    assert "38/38" in text
    assert "22/22" in text
    assert "--where-symbol" in text


def test_compatibility_stub_points_at_canonical_truth() -> None:
    # 521b42a4 collapsed the legacy ./plugin loader into a deprecation stub;
    # the stub's only remaining truth is the pointer at the canonical plugin.
    text = (REPO_ROOT / "plugin/README.md").read_text(encoding="utf-8")
    assert "DEPRECATED" in text
    assert "loctree-plugin/" in text
    assert "one canonical plugin" in text
