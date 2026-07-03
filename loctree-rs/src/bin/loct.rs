use std::any::Any;
use std::panic;

use loctree::cli::entrypoint::{EntryOptions, run};

fn install_broken_pipe_handler() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let is_broken = <dyn Any>::downcast_ref::<&str>(payload)
            .is_some_and(|s| s.contains("Broken pipe"))
            || <dyn Any>::downcast_ref::<String>(payload)
                .is_some_and(|s| s.contains("Broken pipe"));

        if is_broken {
            // Quietly exit when downstream closes the pipe (e.g. piping to `head`).
            std::process::exit(0);
        }

        default_hook(info);
    }));
}

fn main() -> std::io::Result<()> {
    install_broken_pipe_handler();

    run(&EntryOptions {
        binary_name: "loct",
        deprecated: false,
        show_banner: true,
        usage: USAGE,
    })
}

const USAGE: &str = "loct - Static Analysis for AI Agents (v0.8.x)\n\n\
PHILOSOPHY: Scan the WHOLE repo once with `loct auto`, then query with subcommands.\n\
            Artifacts live in your cache dir by default (override with LOCT_CACHE_DIR).\n\n\
Quick Start:\n  \
  loct auto                      Full scan → cached artifacts\n  \
  loct slice src/foo.ts          Extract file context for AI agent\n  \
  loct report --html out.html    Generate visual HTML report\n\n\
Core Commands:\n  \
  auto              Full scan + findings (writes artifacts)\n  \
  doctor            Interactive diagnostics and quick-wins\n  \
  findings          Emit canonical findings JSON\n  \
  slice <file>      Extract file + dependencies + consumers\n  \
  find <name>       Find symbol definitions\n  \
  trace <handler>   Debug Tauri handler pipeline\n  \
  --for-ai          AI-optimized project summary (JSON)\n\n\
Analysis:\n  \
  dead              Find unused exports (dead code)\n  \
  cycles            Find circular imports\n  \
  twins             Find duplicate symbol names\n  \
  health            Quick structural health check\n  \
  crowds            Find hub files (high import/export counts)\n\n\
Output:\n  \
  findings          Full findings JSON / summary JSON\n  \
  report            Generate HTML/JSON/SARIF reports\n  \
  --json            Machine-readable output\n  \
  --sarif           SARIF for GitHub Code Scanning\n\n\
Common:\n  \
  -g, --gitignore   Respect .gitignore\n  \
  --verbose         Detailed progress\n  \
  --help-full       Complete command reference\n\n\
Examples:\n  \
  loct auto                                  # Full analysis\n  \
  loct slice src/main.rs --consumers         # Context for AI\n  \
  loct findings --summary | jq '.health_score' # CI summary JSON\n  \
  loct dead --confidence high                # Find dead code\n  \
  loct report --html out.html --serve        # Interactive report\n  \
  loct doctor                                # Interactive fixes\n\n\
Tip: Run `loct auto` from repo root first, then query!\n\n\
More: loct --help-full\n";
