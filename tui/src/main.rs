mod app;
mod code_editor;
mod components;
mod events;
mod toolbar;
mod ui;

use app::App;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "request-pilot", about = "Request Pilot — HTTP testing from the terminal")]
struct Cli {
    /// .http file(s) to load on startup
    files: Vec<PathBuf>,

    /// .env file to load variables from
    #[arg(short, long, value_name = "ENV")]
    env: Option<PathBuf>,

    /// Run all tests immediately after loading
    #[arg(short, long)]
    run: bool,
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();
    let mut app = App::new();

    // Load .env file if provided
    if let Some(env_path) = &cli.env {
        app.load_env(env_path)?;
    }

    // Load .http files
    for path in &cli.files {
        app.load_file(path)?;
    }

    // Auto-run if requested
    if cli.run && !app.loaded_files.is_empty() {
        app.queue_run_all();
    }

    // Enter TUI
    let terminal = ui::init_terminal()?;
    let result = app.run(terminal).await;
    ui::restore_terminal()?;
    result
}