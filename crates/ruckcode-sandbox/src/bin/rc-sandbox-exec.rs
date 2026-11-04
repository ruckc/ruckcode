//! Standalone launcher entry point (the `rc` binary embeds the same one as `rc __sandbox-exec`).
fn main() {
    ruckcode_sandbox::launcher::maybe_run();
    eprintln!(
        "usage: rc-sandbox-exec {} --policy-fd N -- cmd...",
        ruckcode_sandbox::launcher::SUBCOMMAND
    );
    std::process::exit(2);
}
