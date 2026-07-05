# TypeScript Lint (`ts-lint`)

Detects TypeScript anti-patterns that weaken type safety.

## Rules

| Rule ID | Pattern | Default Severity |
|---------|---------|------------------|
| `ts/explicit-any` | `: any`, `as any`, `<any>`, `any[]` | HIGH (prod) / LOW (test) |
| `ts/ts-ignore` | `@ts-ignore` | HIGH |
| `ts/ts-expect-error` | `@ts-expect-error` | MEDIUM |
| `ts/ts-nocheck` | `@ts-nocheck` | HIGH |

## Severity Logic

- **Production code**: Issues are reported with their base severity
- **Test files**: `HIGH` severity is downgraded to `LOW`

Test file detection patterns:
- `*.test.ts`, `*.spec.ts`
- `__tests__/` directory
- `/test/`, `/tests/` directories
- `test-utils/` directory
- `setup.ts`, `setup.tsx`

## Usage

### Via `--for-ai` (recommended)

```bash
loct --for-ai | jq '.bundle.ts_lint'
```

Output:
```json
[
  {
    "file": "src/services/api.ts",
    "line": 42,
    "rule": "ts/explicit-any",
    "severity": "high",
    "message": "Explicit `any` type weakens type safety"
  }
]
```

### Via `findings.json`

```bash
loct findings
jq '.ts_lint' .loctree/findings.json
```

## Summary Statistics

Available in `.summary.ts_lint`:

```json
{
  "total_issues": 238,
  "by_severity": {
    "high": 180,
    "medium": 10,
    "low": 48
  },
  "by_rule": {
    "ts/explicit-any": 230,
    "ts/ts-ignore": 3,
    "ts/ts-expect-error": 5
  },
  "affected_files": 95,
  "test_files_issues": 48,
  "prod_files_issues": 190
}
```

## Examples

### Bad Code (flagged)

```typescript
// ts/explicit-any
function process(data: any) { ... }
const value = response as any;
const items: any[] = [];
const map = new Map<any, string>();

// ts/ts-ignore
// @ts-ignore
const x = badCode();

// ts/ts-nocheck
// @ts-nocheck
// (disables checking for entire file)
```

### Good Code (not flagged)

```typescript
// Proper types
function process(data: UserData) { ... }
const value = response as UserResponse;
const items: string[] = [];

// unknown is type-safe
function handle(error: unknown) { ... }

// Words containing "any" are fine
const company = "Acme";
const anyway = true;
```

## Integration with CI

Add to your CI pipeline:

```bash
# Fail if more than N high-severity issues in prod code
HIGH_COUNT=$(loct --for-ai | jq '[.bundle.ts_lint[] | select(.severity == "high")] | length')
if [ "$HIGH_COUNT" -gt 10 ]; then
  echo "Too many type safety issues: $HIGH_COUNT"
  exit 1
fi
```

## Limitations

- Pattern-based detection (not full AST analysis)
- May have false positives in comments or strings
- Does not detect implicit `any` (use `noImplicitAny` in tsconfig)

## Related

- [react-lint.md](./react-lint.md) - React-specific lint rules
- TypeScript's `strict` mode for compile-time type checking

---

Developed by The Loctree Team ⓒ 2025-2026.
