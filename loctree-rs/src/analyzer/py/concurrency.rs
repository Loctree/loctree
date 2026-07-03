//! Python concurrency pattern detection for race conditions.
//!
//! Detects threading, asyncio, and multiprocessing patterns that may indicate
//! potential race conditions or concurrent access issues.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use crate::types::PyRaceIndicator;

use super::stdlib::THREAD_SAFE_CONSTRUCTORS;

/// Detect Python concurrency patterns that may indicate race conditions.
pub(super) fn detect_py_race_indicators(content: &str) -> Vec<PyRaceIndicator> {
    let mut indicators = Vec::new();
    let mut has_threading_import = false;
    let mut has_lock_usage = false;
    let mut has_asyncio_import = false;
    let mut has_multiprocessing_import = false;
    let mut has_queue_import = false;
    let mut has_thread_safe_container = false;
    let mut thread_creations: Vec<usize> = Vec::new();
    let mut asyncio_parallel: Vec<(usize, &str)> = Vec::new();
    let mut mp_pool_usage: Vec<usize> = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let line_1based = line_num + 1;
        let trimmed = line.trim();

        // Track imports
        if trimmed.contains("import threading") || trimmed.contains("from threading") {
            has_threading_import = true;
        }
        if trimmed.contains("import asyncio") || trimmed.contains("from asyncio") {
            has_asyncio_import = true;
        }
        if trimmed.contains("import multiprocessing") || trimmed.contains("from multiprocessing") {
            has_multiprocessing_import = true;
        }
        if trimmed.contains("import queue") || trimmed.contains("from queue") {
            has_queue_import = true;
        }

        // Track Lock usage
        if trimmed.contains("Lock(") || trimmed.contains("RLock(") || trimmed.contains("Semaphore(")
        {
            has_lock_usage = true;
        }

        // Track thread-safe container usage (queue.Queue, deque, etc.)
        // These provide built-in synchronization, so threading with them is safe
        for pattern in THREAD_SAFE_CONSTRUCTORS {
            if trimmed.contains(pattern) {
                has_thread_safe_container = true;
                break;
            }
        }
        // Also check for queue usage when queue is imported
        if has_queue_import
            && (trimmed.contains("Queue(")
                || trimmed.contains("LifoQueue(")
                || trimmed.contains("PriorityQueue(")
                || trimmed.contains("SimpleQueue("))
        {
            has_thread_safe_container = true;
        }

        // Track Thread creation
        if trimmed.contains("Thread(")
            && (has_threading_import || trimmed.contains("threading.Thread"))
        {
            thread_creations.push(line_1based);
        }

        // Track asyncio parallel patterns
        if trimmed.contains("asyncio.gather(") || trimmed.contains("gather(") && has_asyncio_import
        {
            asyncio_parallel.push((line_1based, "gather"));
        }
        if trimmed.contains("asyncio.create_task(")
            || trimmed.contains("create_task(") && has_asyncio_import
        {
            asyncio_parallel.push((line_1based, "create_task"));
        }
        if trimmed.contains("asyncio.wait(") || trimmed.contains(".wait(") && has_asyncio_import {
            asyncio_parallel.push((line_1based, "wait"));
        }

        // Track concurrent.futures import
        if trimmed.contains("concurrent.futures") || trimmed.contains("from concurrent") {
            has_multiprocessing_import = true; // Treat as multiprocessing-like
        }

        // Track multiprocessing Pool
        if (trimmed.contains("Pool(")
            || trimmed.contains("ProcessPoolExecutor(")
            || trimmed.contains("ThreadPoolExecutor("))
            && (has_multiprocessing_import
                || trimmed.contains("multiprocessing.")
                || trimmed.contains("concurrent.futures"))
        {
            mp_pool_usage.push(line_1based);
        }
    }

    // Generate warnings based on patterns

    // Threading without Lock - but skip if using thread-safe containers
    // Thread-safe containers (queue.Queue, etc.) have built-in synchronization
    if !thread_creations.is_empty() && !has_lock_usage && !has_thread_safe_container {
        for line in thread_creations {
            indicators.push(PyRaceIndicator {
                line,
                concurrency_type: "threading".to_string(),
                pattern: "Thread".to_string(),
                risk: "warning".to_string(),
                message: "Thread created without Lock/RLock/Semaphore - potential race condition"
                    .to_string(),
            });
        }
    }

    // Asyncio parallel execution (info level - needs manual review)
    for (line, pattern) in asyncio_parallel {
        indicators.push(PyRaceIndicator {
            line,
            concurrency_type: "asyncio".to_string(),
            pattern: pattern.to_string(),
            risk: "info".to_string(),
            message: format!(
                "Parallel async execution with {} - verify shared state access",
                pattern
            ),
        });
    }

    // Multiprocessing pool (info level)
    for line in mp_pool_usage {
        indicators.push(PyRaceIndicator {
            line,
            concurrency_type: "multiprocessing".to_string(),
            pattern: "Pool".to_string(),
            risk: "info".to_string(),
            message: "Process/Thread pool - ensure shared resources are process-safe".to_string(),
        });
    }

    indicators
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_threading_without_lock() {
        let content = r#"
import threading

def worker():
    pass

t = threading.Thread(target=worker)
t.start()
"#;
        let indicators = detect_py_race_indicators(content);
        assert_eq!(indicators.len(), 1);
        assert_eq!(indicators[0].concurrency_type, "threading");
        assert_eq!(indicators[0].risk, "warning");
    }

    #[test]
    fn no_warning_with_lock() {
        let content = r#"
import threading

lock = threading.Lock()

def worker():
    with lock:
        pass

t = threading.Thread(target=worker)
t.start()
"#;
        let indicators = detect_py_race_indicators(content);
        // Should not have threading warning because Lock is used
        let threading_warnings = indicators
            .iter()
            .filter(|i| i.concurrency_type == "threading")
            .count();
        assert_eq!(threading_warnings, 0);
    }

    #[test]
    fn detects_asyncio_gather() {
        let content = r#"
import asyncio

async def main():
    await asyncio.gather(task1(), task2())
"#;
        let indicators = detect_py_race_indicators(content);
        let asyncio_indicators: Vec<_> = indicators
            .iter()
            .filter(|i| i.concurrency_type == "asyncio")
            .collect();
        assert!(!asyncio_indicators.is_empty());
        assert_eq!(asyncio_indicators[0].pattern, "gather");
    }

    #[test]
    fn detects_asyncio_create_task() {
        let content = r#"
import asyncio

async def main():
    task = asyncio.create_task(worker())
"#;
        let indicators = detect_py_race_indicators(content);
        let asyncio_indicators: Vec<_> = indicators
            .iter()
            .filter(|i| i.concurrency_type == "asyncio")
            .collect();
        assert!(!asyncio_indicators.is_empty());
        assert!(
            asyncio_indicators
                .iter()
                .any(|i| i.pattern == "create_task")
        );
    }

    #[test]
    fn detects_multiprocessing_pool() {
        let content = r#"
import multiprocessing

def main():
    with multiprocessing.Pool(4) as pool:
        results = pool.map(worker, data)
"#;
        let indicators = detect_py_race_indicators(content);
        let mp_indicators: Vec<_> = indicators
            .iter()
            .filter(|i| i.concurrency_type == "multiprocessing")
            .collect();
        assert!(!mp_indicators.is_empty());
    }

    #[test]
    fn detects_concurrent_futures_pool() {
        let content = r#"
from concurrent.futures import ThreadPoolExecutor

with ThreadPoolExecutor(max_workers=4) as executor:
    results = executor.map(worker, data)
"#;
        let indicators = detect_py_race_indicators(content);
        let pool_indicators: Vec<_> = indicators.iter().filter(|i| i.pattern == "Pool").collect();
        assert!(!pool_indicators.is_empty());
    }

    #[test]
    fn no_indicators_for_clean_code() {
        let content = r#"
def add(a, b):
    return a + b

result = add(1, 2)
print(result)
"#;
        let indicators = detect_py_race_indicators(content);
        assert!(indicators.is_empty());
    }

    #[test]
    fn detects_asyncio_wait() {
        let content = r#"
import asyncio

async def main():
    done, pending = await asyncio.wait(tasks)
"#;
        let indicators = detect_py_race_indicators(content);
        let asyncio_indicators: Vec<_> = indicators
            .iter()
            .filter(|i| i.concurrency_type == "asyncio")
            .collect();
        assert!(!asyncio_indicators.is_empty());
        assert!(asyncio_indicators.iter().any(|i| i.pattern == "wait"));
    }

    #[test]
    fn no_warning_with_queue() {
        // queue.Queue is thread-safe, so no race warning should be emitted
        let content = r#"
import queue
import threading

class Worker:
    def __init__(self):
        self.queue = queue.Queue()  # Thread-safe

    def start(self):
        threading.Thread(target=self._process).start()

    def _process(self):
        item = self.queue.get()
"#;
        let indicators = detect_py_race_indicators(content);
        let threading_warnings = indicators
            .iter()
            .filter(|i| i.concurrency_type == "threading" && i.risk == "warning")
            .count();
        assert_eq!(
            threading_warnings, 0,
            "queue.Queue is thread-safe, should not warn"
        );
    }

    #[test]
    fn no_warning_with_deque() {
        // collections.deque append/pop are atomic
        let content = r#"
import threading
from collections import deque

class Worker:
    def __init__(self):
        self.tasks = deque()  # Atomic append/pop

    def start(self):
        threading.Thread(target=self._process).start()

    def _process(self):
        self.tasks.append(1)
"#;
        let indicators = detect_py_race_indicators(content);
        let threading_warnings = indicators
            .iter()
            .filter(|i| i.concurrency_type == "threading" && i.risk == "warning")
            .count();
        assert_eq!(threading_warnings, 0, "deque is thread-safe for append/pop");
    }

    #[test]
    fn no_warning_with_multiprocessing_queue() {
        // multiprocessing.Queue is thread-safe
        let content = r#"
import multiprocessing
import threading

class Worker:
    def __init__(self):
        self.queue = multiprocessing.Queue()  # Thread-safe

    def start(self):
        threading.Thread(target=self._process).start()

    def _process(self):
        item = self.queue.get()
"#;
        let indicators = detect_py_race_indicators(content);
        let threading_warnings = indicators
            .iter()
            .filter(|i| i.concurrency_type == "threading" && i.risk == "warning")
            .count();
        assert_eq!(
            threading_warnings, 0,
            "multiprocessing.Queue is thread-safe"
        );
    }

    #[test]
    fn warning_with_unsafe_list() {
        // Plain list is NOT thread-safe, should warn
        let content = r#"
import threading

class Worker:
    def __init__(self):
        self.items = []  # NOT thread-safe

    def start(self):
        threading.Thread(target=self._process).start()

    def _process(self):
        self.items.append(1)
"#;
        let indicators = detect_py_race_indicators(content);
        let threading_warnings = indicators
            .iter()
            .filter(|i| i.concurrency_type == "threading" && i.risk == "warning")
            .count();
        assert_eq!(
            threading_warnings, 1,
            "list is NOT thread-safe, should warn"
        );
    }

    #[test]
    fn no_warning_with_direct_queue_import() {
        // Direct import: from queue import Queue
        let content = r#"
from queue import Queue
import threading

class Worker:
    def __init__(self):
        self.queue = Queue()  # Thread-safe

    def start(self):
        threading.Thread(target=self._process).start()

    def _process(self):
        item = self.queue.get()
"#;
        let indicators = detect_py_race_indicators(content);
        let threading_warnings = indicators
            .iter()
            .filter(|i| i.concurrency_type == "threading" && i.risk == "warning")
            .count();
        assert_eq!(
            threading_warnings, 0,
            "Queue (direct import) is thread-safe"
        );
    }
}
