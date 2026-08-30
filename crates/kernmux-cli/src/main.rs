fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(
        kernmux_cli::run(std::env::args_os(), std::io::stdout(), std::io::stderr()).value(),
    )
}
