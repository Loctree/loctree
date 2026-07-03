use std::any::Any;
use std::io::IsTerminal;
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
            std::process::exit(0);
        }

        default_hook(info);
    }));
}

fn main() -> std::io::Result<()> {
    install_broken_pipe_handler();

    // Deprecation warning — stderr only, interactive only, suppressed in JSON/quiet mode.
    // SAFETY: CLI trust boundary; args are inspected only for the literal flags `--json`,
    // `--quiet`, `-q` to decide whether to print the deprecation banner. No path, no exec,
    // no security decision touches these strings. Rule suppression is enforced at file
    // scope via `.semgrepignore` (CLI ENTRY POINTS).
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let wants_json = raw_args.iter().any(|a| a == "--json");
    let quiet = raw_args.iter().any(|a| a == "--quiet" || a == "-q");
    let interactive = std::io::stderr().is_terminal();
    if interactive && !wants_json && !quiet {
        eprintln!("\x1b[1;33m");
        eprintln!("  DEPRECATED: `loctree` will be removed in v1.0");
        eprintln!("  Use `loct` instead — same features, shorter name");
        eprintln!("\x1b[0m");
    }

    run(&EntryOptions {
        binary_name: "loctree",
        deprecated: true,
        show_banner: false,
        usage: USAGE,
    })
}

const USAGE: &str = "loctree (DEPRECATED - use `loct` instead)\n\n\
This binary will be removed in v1.0. All commands work with `loct`.\n\n\
Migration Guide:\n  \
  loctree                    ->  loct auto\n  \
  loctree -A --dead          ->  loct dead\n  \
  loctree -A --circular      ->  loct cycles\n  \
  loctree -A --report f.html ->  loct report --html f.html\n  \
  loctree slice file         ->  loct slice file\n  \
  loctree --for-ai           ->  loct --for-ai\n\n\
New Features in `loct`:\n  \
  loct auto              Full scan + findings (cached artifacts; set LOCT_CACHE_DIR to override)\n  \
  loct doctor            Interactive diagnostics\n  \
  loct findings          Canonical findings JSON\n  \
  loct dead              Find unused exports\n  \
  loct cycles            Find circular imports\n  \
  loct twins             Find duplicate symbols\n  \
  loct health            Quick health check\n  \
  loct find <name>       Find symbol definitions\n\n\
Run `loct --help` for full documentation.\n";
