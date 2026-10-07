use clap::Parser;
use std::io::stdout;
use std::process::ExitCode;

fn main() -> ExitCode {
    env_logger::init();
    let cli = vimanam::cli::Cli::parse();
    match vimanam::cli::run(&cli, &mut stdout(), &mut |notice| eprintln!("{notice}")) {
        Ok(true) => ExitCode::from(3),
        Ok(false) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::from(1)
        }
    }
}
