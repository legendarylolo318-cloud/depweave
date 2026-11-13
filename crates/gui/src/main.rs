mod app;
mod export;
mod panels;
mod watch;

slint::include_modules!();

fn main() {
    use clap::Parser;
    let cli = depweave_cli::Cli::parse();
    if cli.command.is_some() {
        std::process::exit(depweave_cli::run(cli));
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(e) = app::run(cli.dbpath) {
        eprintln!("depweave: cannot start the UI: {e}");
        std::process::exit(1);
    }
}
