# Contributing to Loctree

## Git Hooks Setup

After cloning, run once (or use `make install` which includes this):

```bash
make git-hooks
```

This creates symlinks from `.git/hooks/` to `tools/hooks/`, ensuring all contributors run the same checks (clippy, tests, formatting) before pushing.

## Development Workflow

```bash
cargo build              # Build
cargo test               # Run tests
cargo clippy             # Lint
cargo fmt                # Format
make precheck            # Quick validation (fmt + clippy + check)
```

## Pull Requests

- Run `make precheck` before submitting
- Keep commits atomic and well-described
- Update docs if adding new features

---

*Developed with care by The Loctree Team ⓒ 2025-2026*
