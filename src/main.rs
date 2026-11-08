mod app;
mod config;
mod format;
mod input;
mod keymap;
mod model;
mod qr;
mod td;
mod theme;
mod ui;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, EventStream};
use crossterm::execute;
use futures::StreamExt;

use crate::app::App;
use crate::keymap::Keymap;
use crate::td::{Event, Td};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {e}", config::APP_NAME);
            ExitCode::FAILURE
        }
    }
}

fn prepare_data_dir() -> Result<std::path::PathBuf, String> {
    let dir = config::data_dir()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot set permissions on {}: {e}", dir.display()))?;
    Ok(dir)
}

fn load_keymap(warnings: &mut Vec<String>) -> Result<Keymap, String> {
    let path = config::keymap_path()?;
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            Keymap::from_toml(&raw, warnings).map_err(|e| format!("{}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Keymap::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

async fn run() -> Result<(), String> {
    let creds = config::load_credentials()?;
    let data_dir = prepare_data_dir()?;
    let mut warnings = Vec::new();
    let keymap = load_keymap(&mut warnings)?;
    td::configure_logging(&data_dir.join("tdlib.log"))?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    td::spawn_receiver(tx.clone());
    let td = Td::new(tx.clone());
    td.start();

    let mut app = App::new(td, creds, data_dir, keymap);
    if !warnings.is_empty() {
        app.set_status(format!("keymap: {}", warnings.join("; ")), true);
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let result = event_loop(&mut terminal, &mut app, &mut rx).await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> Result<(), String> {
    let mut input = EventStream::new();
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        if app.force_clear {
            app.force_clear = false;
            terminal.clear().map_err(|e| format!("terminal: {e}"))?;
        }
        terminal
            .draw(|f| ui::draw(f, app))
            .map_err(|e| format!("terminal: {e}"))?;
        app.after_draw();
        if app.exit_ready {
            return Ok(());
        }
        if app.quitting && deadline.is_none() {
            deadline = Some(tokio::time::Instant::now() + Duration::from_secs(5));
        }
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(ev)) => app.handle(Event::Input(ev)),
                Some(Err(e)) => return Err(format!("terminal input: {e}")),
                None => return Err("terminal input closed".to_owned()),
            },
            ev = rx.recv() => match ev {
                Some(ev) => app.handle(ev),
                None => return Err("TDLib receiver stopped".to_owned()),
            },
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                return Err("TDLib did not close within 5 s; database may need recovery on next start".to_owned());
            }
        }
        while let Ok(ev) = rx.try_recv() {
            app.handle(ev);
        }
    }
}
