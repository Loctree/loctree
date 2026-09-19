use std::any::Any;
use std::panic;

use loctree::cli::command::LOCT_USAGE;
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
        usage: LOCT_USAGE,
    })
}
