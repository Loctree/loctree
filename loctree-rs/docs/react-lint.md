# React Lint: Race Condition Detection

> Static analysis for React race conditions using OXC AST parser.
> Zero runtime overhead. Zero dependencies. Pure Rust.

---

## Overview

React's `useEffect` hook is deceptively simple but notoriously easy to misuse. Async operations without proper cleanup cause:

- **Memory leaks** - callbacks firing after unmount
- **Race conditions** - stale data overwriting fresh data
- **Zombie state updates** - `setState` on unmounted components
- **Unpredictable UI** - flickering, wrong data, crashes

**React Lint** statically detects these patterns at build time, before they reach production.

---

## Quick Start

```bash
# From your React project directory
loct findings

# Output includes react_lint section:
# {
#   "react_lint": [
#     {
#       "file": "src/hooks/useData.ts",
#       "line": 42,
#       "rule": "react/async-effect-no-cleanup",
#       "severity": "high",
#       "message": "Async useEffect without cleanup...",
#       "suggestion": "Add a cleanup function with cancelled flag..."
#     }
#   ]
# }
```

---

## Detection Rules

### `react/async-effect-no-cleanup` (HIGH)

Detects async operations in useEffect without cleanup mechanism.

```tsx
// BAD - will flag
useEffect(() => {
  const fetchData = async () => {
    const data = await api.getData();
    setData(data);  // May fire after unmount!
  };
  fetchData();
}, []);

// GOOD - won't flag
useEffect(() => {
  let cancelled = false;

  const fetchData = async () => {
    const data = await api.getData();
    if (!cancelled) setData(data);
  };
  fetchData();

  return () => { cancelled = true; };
}, []);
```

**Why it matters:** If component unmounts while `fetchData` is in-flight, `setData` fires on an unmounted component. React warns about this, and it can cause memory leaks.

---

### `react/settimeout-no-cleanup` (MEDIUM)

Detects `setTimeout` calls without corresponding `clearTimeout`.

```tsx
// BAD - will flag
useEffect(() => {
  setTimeout(() => {
    setVisible(true);
  }, 1000);
}, []);

// GOOD - won't flag
useEffect(() => {
  const timer = setTimeout(() => {
    setVisible(true);
  }, 1000);

  return () => clearTimeout(timer);
}, []);
```

---

### `react/setinterval-no-cleanup` (HIGH)

Detects `setInterval` calls without corresponding `clearInterval`.

```tsx
// BAD - will flag (and it's really bad)
useEffect(() => {
  setInterval(() => {
    tick();
  }, 100);
}, []);

// GOOD - won't flag
useEffect(() => {
  const interval = setInterval(() => tick(), 100);
  return () => clearInterval(interval);
}, []);
```

**Why HIGH severity:** Unlike setTimeout, intervals keep firing forever. Missing cleanup = guaranteed memory leak.

---

### `react/event-listener-no-cleanup` (MEDIUM)

Detects `addEventListener` without corresponding `removeEventListener`.

```tsx
// BAD - will flag
useEffect(() => {
  window.addEventListener('resize', handleResize);
}, []);

// GOOD - won't flag
useEffect(() => {
  window.addEventListener('resize', handleResize);
  return () => window.removeEventListener('resize', handleResize);
}, []);
```

---

## Recognized Cleanup Patterns

The analyzer understands these cleanup idioms and **won't flag** code that uses them:

### Cancelled Flag Pattern
```tsx
useEffect(() => {
  let cancelled = false;
  // ...async work...
  return () => { cancelled = true; };
}, []);
```

### Mounted Flag Pattern
```tsx
useEffect(() => {
  let mounted = true;
  // ...
  return () => { mounted = false; };
}, []);
```

### Ref-based Pattern
```tsx
const isMountedRef = useRef(true);
useEffect(() => {
  // ...checks isMountedRef.current...
  return () => { isMountedRef.current = false; };
}, []);
```

### AbortController Pattern
```tsx
useEffect(() => {
  const controller = new AbortController();
  fetch(url, { signal: controller.signal });
  return () => controller.abort();
}, []);
```

### Timer Cleanup
```tsx
useEffect(() => {
  const timer = setTimeout(...);
  return () => clearTimeout(timer);
}, []);
```

### Event Listener Cleanup
```tsx
useEffect(() => {
  window.addEventListener('event', handler);
  return () => window.removeEventListener('event', handler);
}, []);
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    react_lint.rs                        │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌─────────────┐    ┌──────────────┐    ┌───────────┐  │
│  │  OXC Parse  │───▶│  AST Visit   │───▶│  Analyze  │  │
│  │  (fast!)    │    │  useEffect   │    │  cleanup  │  │
│  └─────────────┘    └──────────────┘    └───────────┘  │
│         │                  │                  │         │
│         ▼                  ▼                  ▼         │
│  ┌─────────────────────────────────────────────────┐   │
│  │              ReactLintIssue[]                   │   │
│  │  • file, line, column                           │   │
│  │  • rule (enum)                                  │   │
│  │  • severity (high/medium)                       │   │
│  │  • message + suggestion                         │   │
│  └─────────────────────────────────────────────────┘   │
│                                                         │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
              ┌─────────────────────────┐
              │     findings.json       │
              │  (consolidated output)  │
              └─────────────────────────┘
```

### Why OXC?

- **Speed**: 3-5x faster than tree-sitter for JS/TS
- **Accuracy**: Full TypeScript support, JSX-aware
- **Rust-native**: No FFI overhead, compiles with loctree
- **Battle-tested**: Used by Biome, oxlint, and other production tools

---

## Integration with Findings

React lint results are included in the standard `findings.json` output:

```json
{
  "summary": {
    "react_lint": {
      "total_issues": 13,
      "by_severity": {
        "high": 10,
        "medium": 3
      },
      "by_rule": {
        "react/async-effect-no-cleanup": 10,
        "react/settimeout-no-cleanup": 2,
        "react/setinterval-no-cleanup": 1
      },
      "affected_files": 8
    }
  },
  "react_lint": [
    {
      "file": "src/contexts/AuthContext.tsx",
      "line": 157,
      "column": 3,
      "rule": "react/async-effect-no-cleanup",
      "severity": "high",
      "message": "Async useEffect without cleanup. May cause setState on unmounted component.",
      "suggestion": "Add a cleanup function with a cancelled flag or AbortController."
    }
  ]
}
```

---

## Real-World Validation

Tested across multiple production codebases:

| Project | Type | Files Scanned | Issues Found | False Positives |
|---------|------|---------------|--------------|-----------------|
| Vista (Tauri + React) | Large app | 746 | 9-13* | 0 |
| TheBeingsSpace (Vite) | Medium app | ~200 | 1 | 0 |
| Nextra docs (Next.js) | Docs site | ~50 | 1 | 0 |
| Fresh CNA project | Starter | ~20 | 0 | 0 |

*Varies by branch - older code has more issues*

Every finding was manually verified. **Zero false positives** in testing.

---

## Known Limitations

### Won't Detect

1. **Custom hooks wrapping useEffect**
   ```tsx
   // Won't flag - can't see inside useAsyncEffect
   useAsyncEffect(async () => {
     await fetchData();
   }, []);
   ```

2. **External library patterns**
   - react-query, SWR, Apollo - have built-in cleanup
   - Analyzer doesn't know they're safe

3. **Cleanup in imported functions**
   ```tsx
   // Won't flag - cleanup logic is elsewhere
   useEffect(() => {
     setupSubscription();  // cleanup inside this function
   }, []);
   ```

4. **Non-React frameworks**
   - Vue, Svelte, Angular use different patterns
   - This tool is React-specific

### May Flag (but safe)

1. **Fire-and-forget effects** that intentionally don't cleanup
   ```tsx
   // May flag, but sometimes intentional
   useEffect(() => {
     analytics.track('page_view');  // Don't care if unmounted
   }, []);
   ```

---

## Performance

| Metric | Value |
|--------|-------|
| Parse + analyze 1 file | ~1-2ms |
| Full Vista scan (746 files) | ~2-3s |
| Memory overhead | Negligible |
| Binary size impact | +~50KB |

React lint runs as part of the standard `loct findings` scan. No additional passes required.

---

## Future Roadmap

- [ ] **`react/exhaustive-deps`** - missing dependencies in useEffect
- [ ] **`react/stale-closure`** - closures capturing stale values
- [ ] **Custom hook awareness** - recognize cleanup in common libraries
- [ ] **Auto-fix suggestions** - generate cleanup code
- [ ] **SARIF output** - for IDE/CI integration

---

## API Reference

### `analyze_react_file`

```rust
pub fn analyze_react_file(
    content: &str,
    path: &Path,
    relative_path: String,
) -> Vec<ReactLintIssue>
```

Analyzes a single file for React race conditions.

### `ReactLintIssue`

```rust
pub struct ReactLintIssue {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub rule: String,        // e.g., "react/async-effect-no-cleanup"
    pub severity: String,    // "high" | "medium"
    pub message: String,
    pub suggestion: Option<String>,
}
```

### `ReactLintRule`

```rust
pub enum ReactLintRule {
    AsyncEffectNoCleanup,    // HIGH - async without cleanup
    SetTimeoutNoCleanup,     // MEDIUM - setTimeout without clearTimeout
    SetIntervalNoCleanup,    // HIGH - setInterval without clearInterval
    EventListenerNoCleanup,  // MEDIUM - addEventListener without remove
}
```

---

## Contributing

React lint lives in `src/analyzer/react_lint.rs`. Key areas for contribution:

1. **New rules** - add to `ReactLintRule` enum and visitor logic
2. **Cleanup patterns** - expand pattern recognition in `check_cleanup_patterns`
3. **Tests** - unit tests at bottom of `react_lint.rs` (11 existing)

Run tests:
```bash
cargo test react_lint
```

---

## Credits

Built with:
- [OXC](https://github.com/oxc-project/oxc) - The JavaScript Oxidation Compiler
- Pattern research from React docs, Dan Abramov's blog, and real-world bug hunting
