use tracing_subscriber::EnvFilter;

fn main() {
    let app = std::env::args().nth(1).as_deref() == Some("app");
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // The app protocol owns stdout. Logs have to stay on stderr.
    if app {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .init();
    }

    whoosh::cli::launch();
}
