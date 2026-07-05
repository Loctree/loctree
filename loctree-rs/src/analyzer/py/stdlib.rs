//! Python standard library set and thread-safe constructor patterns.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use std::collections::HashSet;
use std::sync::OnceLock;

/// Returns a reference to the static Python standard library module set.
/// These modules are part of the Python standard library and should not be
/// resolved as local imports.
pub(crate) fn python_stdlib_set() -> &'static HashSet<String> {
    static STDLIB: OnceLock<HashSet<String>> = OnceLock::new();
    STDLIB.get_or_init(|| {
        [
            "abc",
            "argparse",
            "array",
            "asyncio",
            "base64",
            "binascii",
            "bisect",
            "cmath",
            "collections",
            "concurrent",
            "contextlib",
            "copy",
            "crypt",
            "csv",
            "ctypes",
            "dataclasses",
            "datetime",
            "decimal",
            "difflib",
            "email",
            "errno",
            "functools",
            "gc",
            "getpass",
            "glob",
            "hashlib",
            "heapq",
            "html",
            "http",
            "importlib",
            "inspect",
            "io",
            "ipaddress",
            "itertools",
            "json",
            "logging",
            "lzma",
            "math",
            "multiprocessing",
            "numbers",
            "operator",
            "os",
            "pathlib",
            "pickle",
            "platform",
            "plistlib",
            "queue",
            "random",
            "re",
            "sched",
            "secrets",
            "select",
            "shlex",
            "shutil",
            "signal",
            "socket",
            "sqlite3",
            "ssl",
            "statistics",
            "string",
            "struct",
            "subprocess",
            "sys",
            "tempfile",
            "textwrap",
            "threading",
            "time",
            "timeit",
            "tkinter",
            "traceback",
            "types",
            "typing",
            "typing_extensions",
            "unicodedata",
            "urllib",
            "uuid",
            "xml",
            "xmlrpc",
            "zipfile",
            "zlib",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    })
}

/// Thread-safe Python types that don't need race condition warnings.
/// These types have built-in synchronization and are safe for concurrent access.
pub(super) const THREAD_SAFE_CONSTRUCTORS: &[&str] = &[
    // queue module - all queues are thread-safe
    "queue.Queue",
    "queue.LifoQueue",
    "queue.PriorityQueue",
    "queue.SimpleQueue",
    "Queue(", // Direct import: from queue import Queue
    "LifoQueue(",
    "PriorityQueue(",
    "SimpleQueue(",
    // multiprocessing module
    "multiprocessing.Queue",
    "multiprocessing.JoinableQueue",
    "multiprocessing.Manager",
    "multiprocessing.Value",
    "multiprocessing.Array",
    // threading primitives (already recognized as Lock usage)
    "threading.Lock",
    "threading.RLock",
    "threading.Condition",
    "threading.Semaphore",
    "threading.BoundedSemaphore",
    "threading.Event",
    "threading.Barrier",
    // concurrent.futures
    "concurrent.futures.ThreadPoolExecutor",
    "concurrent.futures.ProcessPoolExecutor",
    "ThreadPoolExecutor(",
    "ProcessPoolExecutor(",
    // collections.deque - append/pop are atomic
    "collections.deque",
    "deque(",
    // asyncio queues
    "asyncio.Queue",
    "asyncio.PriorityQueue",
    "asyncio.LifoQueue",
];
