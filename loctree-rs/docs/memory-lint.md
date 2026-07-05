# Memory Lint: Memory Leak Detection

> Static analysis for JavaScript/TypeScript memory leaks OUTSIDE React hooks.
> For React-specific leaks inside `useEffect`, see [react-lint.md](./react-lint.md).

---

## Overview

Memory leaks in JavaScript/TypeScript are insidious - they don't crash immediately but slowly degrade performance. Common sources outside React lifecycle:

- **Module-level caches** - Maps/Sets that grow unbounded
- **Subscriptions** - RxJS/EventEmitter without cleanup
- **Global intervals** - `setInterval` in services without `clearInterval`
- **Event listeners** - `addEventListener` without removal

**Memory Lint** detects these patterns statically, before they reach production.

---

## Quick Start

```bash
# From your project directory
loct findings

# Output includes memory_lint section:
# {
#   "memory_lint": [
#     {
#       "file": "src/services/cache.ts",
#       "line": 5,
#       "rule": "mem/module-cache-unbounded",
#       "severity": "medium",
#       "message": "Module-level Map/Set without size limit..."
#     }
#   ]
# }
```

---

## Detection Rules

### `mem/module-cache-unbounded` (MEDIUM)

Detects `new Map()` or `new Set()` at module level without size limits.

```typescript
// BAD - will flag
const cache = new Map();

export function getData(key: string) {
    if (!cache.has(key)) {
        cache.set(key, fetchData(key));  // Grows forever!
    }
    return cache.get(key);
}

// GOOD - won't flag (has LRU pattern)
import { LRUCache } from 'lru-cache';
const cache = new LRUCache({ max: 100 });

// GOOD - won't flag (has manual eviction)
const cache = new Map();
if (cache.size > 100) {
    cache.delete(cache.keys().next().value);
}
```

**Why it matters:** Module-level caches persist for the entire application lifetime. Without bounds, they can consume all available memory.

---

### `mem/subscription-leak` (HIGH)

Detects `.subscribe()` without corresponding `.unsubscribe()`.

```typescript
// BAD - will flag
import { fromEvent } from 'rxjs';

export function setupListener() {
    fromEvent(document, 'click').subscribe(event => {
        console.log(event);  // Never unsubscribes!
    });
}

// GOOD - won't flag (has unsubscribe)
import { Subscription } from 'rxjs';

let sub: Subscription;

export function setup() {
    sub = fromEvent(document, 'click').subscribe(console.log);
}

export function cleanup() {
    sub.unsubscribe();
}

// GOOD - won't flag (uses takeUntil pattern)
import { takeUntil, Subject } from 'rxjs';

const destroy$ = new Subject();

fromEvent(document, 'click')
    .pipe(takeUntil(destroy$))
    .subscribe(console.log);
```

**Why HIGH severity:** Subscriptions keep firing callbacks even after the subscriber is no longer needed, causing memory retention and stale updates.

---

### `mem/global-interval` (HIGH)

Detects `setInterval` in non-React files without `clearInterval`.

```typescript
// BAD - will flag (in .ts file)
export function startPolling() {
    setInterval(() => {
        fetchData();  // Runs forever!
    }, 5000);
}

// GOOD - won't flag
let intervalId: number;

export function startPolling() {
    intervalId = setInterval(fetchData, 5000);
}

export function stopPolling() {
    clearInterval(intervalId);
}
```

**Note:** In React files (.tsx) with `useEffect`, this rule is skipped - `react-lint` handles those cases.

---

### `mem/global-event-listener` (MEDIUM)

Detects `addEventListener` outside React components without `removeEventListener`.

```typescript
// BAD - will flag (in .ts file)
export function init() {
    window.addEventListener('resize', handleResize);
    // Never removed!
}

// GOOD - won't flag
export function init() {
    window.addEventListener('resize', handleResize);
}

export function cleanup() {
    window.removeEventListener('resize', handleResize);
}
```

---

## Recognized Safe Patterns

The analyzer understands these cleanup idioms and **won't flag** code that uses them:

### LRU Cache Pattern
```typescript
import { LRUCache } from 'lru-cache';
const cache = new LRUCache({ max: 100 });
```

### Manual Eviction Pattern
```typescript
const cache = new Map();
// Detected: .delete() or .clear() in same file
```

### takeUntil Pattern (RxJS)
```typescript
observable.pipe(takeUntil(destroy$)).subscribe(...);
```

### take(1) / first() Pattern
```typescript
observable.pipe(take(1)).subscribe(...);
observable.pipe(first()).subscribe(...);
```

---

## Integration with Findings

Memory lint results are included in the standard `findings.json` output:

```json
{
  "summary": {
    "memory_lint": {
      "total_issues": 5,
      "by_severity": {
        "high": 3,
        "medium": 2
      },
      "by_rule": {
        "mem/subscription-leak": 3,
        "mem/module-cache-unbounded": 2
      },
      "affected_files": 4
    }
  },
  "memory_lint": [
    {
      "file": "src/services/analytics.ts",
      "line": 42,
      "column": 5,
      "rule": "mem/subscription-leak",
      "severity": "high",
      "message": "Subscription created without corresponding unsubscribe - potential memory leak",
      "suggestion": "Store subscription and call .unsubscribe() when done, or use takeUntil pattern"
    }
  ]
}
```

---

## Usage via `--for-ai`

```bash
loct --for-ai | jq '.bundle.memory_lint'
```

Output (high severity only):
```json
[
  {
    "file": "src/services/cache.ts",
    "line": 5,
    "rule": "mem/subscription-leak",
    "severity": "high",
    "message": "Subscription created without corresponding unsubscribe..."
  }
]
```

---

## CI Integration

```bash
# Fail if any high-severity memory leaks
HIGH_COUNT=$(loct --for-ai | jq '[.bundle.memory_lint[] | select(.severity == "high")] | length')
if [ "$HIGH_COUNT" -gt 0 ]; then
  echo "Found $HIGH_COUNT high-severity memory leak patterns"
  exit 1
fi
```

---

## Difference from React Lint

| Aspect | react-lint | memory-lint |
|--------|-----------|-------------|
| Scope | Inside `useEffect` hooks | Outside React lifecycle |
| File types | `.tsx`, `.jsx` | `.ts`, `.js`, `.tsx`, `.jsx` |
| Focus | Race conditions, cleanup | Global leaks, subscriptions |
| setInterval | In useEffect | In services/utils |

**Use both** for comprehensive memory leak detection:
- `react-lint` catches component lifecycle issues
- `memory-lint` catches service/utility layer issues

---

## Limitations

### Won't Detect

1. **Leaks in imported functions**
   ```typescript
   // Won't flag - cleanup logic is hidden
   import { setupPolling } from './utils';
   setupPolling();  // May or may not have cleanup
   ```

2. **Custom observable libraries**
   - Only detects RxJS `.subscribe()/.unsubscribe()` pattern
   - Custom implementations need manual review

3. **WeakMap/WeakSet**
   - These are garbage-collected automatically
   - Not flagged (correctly - they don't leak)

4. **Closures holding references**
   - Requires data flow analysis
   - Too complex for static regex-based detection

### May Flag (but safe)

1. **Intentional singleton caches**
   ```typescript
   // May flag, but sometimes intentional
   const CONFIG = new Map();  // Loaded once, never cleared
   ```

---

## Performance

| Metric | Value |
|--------|-------|
| Parse + analyze 1 file | ~0.5ms |
| Full scan (500 files) | ~1-2s |
| Memory overhead | Negligible |
| Binary size impact | +~20KB |

Memory lint runs as part of the standard `loct findings` scan. No additional passes required.

---

## API Reference

### `lint_memory_file`

```rust
pub fn lint_memory_file(path: &Path, content: &str) -> Vec<MemoryLintIssue>
```

Analyzes a single file for memory leak patterns.

### `MemoryLintIssue`

```rust
pub struct MemoryLintIssue {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub rule: String,        // e.g., "mem/subscription-leak"
    pub severity: String,    // "high" | "medium"
    pub message: String,
    pub suggestion: Option<String>,
}
```

### `MemoryLintRule`

```rust
pub enum MemoryLintRule {
    ModuleCacheUnbounded,  // MEDIUM - unbounded Map/Set
    SubscriptionLeak,      // HIGH - .subscribe without .unsubscribe
    GlobalInterval,        // HIGH - setInterval without clearInterval
    GlobalEventListener,   // MEDIUM - addEventListener without remove
}
```

---

## Related

- [react-lint.md](./react-lint.md) - React-specific race condition detection
- [ts-lint.md](./ts-lint.md) - TypeScript type safety linting

---

Developed by The Loctree Team ⓒ 2025-2026.
