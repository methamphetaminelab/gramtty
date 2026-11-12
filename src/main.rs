mod admin;
mod anim;
mod app;
mod archive;
mod bots;
mod config;
#[cfg(test)]
mod demo;
mod details;
mod format;
mod input;
mod keymap;
mod media;
mod model;
mod notify;
mod player;
mod popups;
mod presence;
mod qr;
mod search;
mod stickers;
mod td;
mod termcolors;
mod termprobe;
mod textmap;
mod theme;
mod ui;
mod upload;
mod video;
mod voice;

use std::process::ExitCode;
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, EventStream, KeyboardEnhancementFlags, MouseEvent,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use futures::StreamExt;

use ratatui_image::picker::{Picker, ProtocolType};

use crate::app::App;
use crate::keymap::Keymap;
use crate::td::{Event, Td};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    #[cfg(windows)]
    if winconsole::relaunch_in_terminal() {
        return ExitCode::SUCCESS;
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {e}", config::APP_NAME);
            #[cfg(windows)]
            if winconsole::owned() {
                eprintln!("Press Enter to close this window");
                let _ = std::io::stdin().read_line(&mut String::new());
            }
            ExitCode::FAILURE
        }
    }
}

fn prepare_data_dir() -> Result<std::path::PathBuf, String> {
    let dir = config::data_dir()?;
    config::create_private_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot set permissions on {}: {e}", dir.display()))?;
    }
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

fn print_theme(args: &[String]) -> Result<(), String> {
    let mut notes = Vec::new();
    let choice = match args {
        [] => {
            config::load_settings(&config::settings_path()?, &mut notes)?
                .appearance
                .theme
        }
        [name] => config::ThemeChoice::ALL
            .into_iter()
            .find(|c| c.name() == name)
            .ok_or_else(|| {
                format!(
                    "unknown theme {name:?}; use one of {}",
                    config::ThemeChoice::ALL.map(|c| c.name()).join(", ")
                )
            })?,
        _ => {
            return Err(
                "usage: --print-theme [terminal|default|high-contrast|light|mono|custom]"
                    .to_owned(),
            );
        }
    };
    let theme = theme::load(choice, &config::theme_path()?, &mut notes)?;
    for note in notes {
        eprintln!("{note}");
    }
    print!("{}", theme.to_toml(choice)?);
    Ok(())
}

fn help_text() -> String {
    let name = config::APP_NAME;
    let prefix = config::env_prefix();
    let dir = |d: Result<std::path::PathBuf, String>| {
        d.map_or_else(|e| format!("({e})"), |p| p.display().to_string())
    };
    format!(
        "{name} {version}: a terminal Telegram client\n\n\
         Usage: {name} [option]\n\n\
         Options:\n  \
         -h, --help                 show this help\n  \
         -V, --version              show the version\n  \
         --print-theme [preset]     print a theme as TOML (terminal, default, high-contrast, light, mono, custom)\n  \
         --print-keymap             print every key binding with its action name\n\n\
         Environment:\n  \
         {keys:<34} your own Telegram API keys\n  \
         {data_var:<34} where the session and data live\n  \
         {proto_var:<34} auto, kitty, sixel, iterm2 or halfblocks\n\n\
         Config: {config}\n\
         Data:   {data}\n\n\
         Inside the app press ? to see all key bindings, Space for the leader menu.\n",
        version = env!("CARGO_PKG_VERSION"),
        keys = format!("{prefix}_API_ID, {prefix}_API_HASH"),
        data_var = format!("{prefix}_DATA_DIR"),
        proto_var = format!("{prefix}_IMAGE_PROTOCOL"),
        config = dir(config::config_dir()),
        data = dir(config::data_dir()),
    )
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        None => {}
        Some((flag, _)) if flag == "--version" || flag == "-V" => {
            println!("{} {}", config::APP_NAME, env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some((flag, _)) if flag == "--help" || flag == "-h" => {
            print!("{}", help_text());
            return Ok(());
        }
        Some((flag, rest)) if flag == "--print-theme" => {
            probe_terminal(false)?;
            return print_theme(rest);
        }
        Some((flag, _)) if flag == "--print-keymap" => {
            let mut warnings = Vec::new();
            let keymap = load_keymap(&mut warnings)?;
            for warning in warnings {
                eprintln!("{warning}");
            }
            print!("{}", keymap.help_text());
            return Ok(());
        }
        Some((other, _)) => {
            return Err(format!(
                "unknown argument {other:?}; see {} --help",
                config::APP_NAME
            ));
        }
    }
    let creds = config::load_credentials()?;
    let probe = probe_terminal(true)?;
    let data_dir = prepare_data_dir()?;
    let mut warnings = Vec::new();
    let keymap = load_keymap(&mut warnings)?;
    let settings_path = config::settings_path()?;
    let settings = config::load_settings(&settings_path, &mut warnings)?;
    let theme_path = config::theme_path()?;
    let theme = theme::load(settings.appearance.theme, &theme_path, &mut warnings)?;
    td::configure_logging(&data_dir.join("tdlib.log"))?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    td::spawn_receiver(tx.clone());
    let td = Td::new(tx.clone());
    td.start();

    drop(ratatui::init());
    release_mouse_on_panic();
    let mut terminal = match ratatui::Terminal::new(ui::Resync(
        ratatui::backend::CrosstermBackend::new(std::io::stdout()),
    )) {
        Ok(t) => t,
        Err(e) => {
            ratatui::restore();
            return Err(format!("terminal: {e}"));
        }
    };
    let _ = execute!(
        std::io::stdout(),
        EnableBracketedPaste,
        EnableFocusChange,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let tmux = probe.image.as_ref().is_some_and(|i| i.tmux);
    let (picker, picker_warning) = image_picker(probe);
    let native_animation = picker.protocol_type() == ProtocolType::Kitty
        && !tmux
        && anim::kitty_terminal(
            std::env::var("TERM").ok().as_deref(),
            std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
        );
    let redrawn_animation = matches!(
        picker.protocol_type(),
        ProtocolType::Sixel | ProtocolType::Iterm2 | ProtocolType::Halfblocks
    );
    let mut app = App::new(td, creds, data_dir, keymap, picker);
    if native_animation || redrawn_animation {
        match config::cache_dir() {
            Ok(dir) if native_animation => app.media.anim.enable(dir.join("anim")),
            Ok(dir) => app.media.anim.enable_redraw(dir.join("anim")),
            Err(e) => warnings.push(format!("inline animations are off: {e}")),
        }
    }
    if native_animation {
        app.inline.enable();
    }
    app.settings = settings;
    app.settings_path = Some(settings_path);
    app.set_theme(theme);
    let watch_tx = tx.clone();
    if let Err(e) = theme::watch(&theme_path, move |r| {
        let _ = watch_tx.send(Event::ThemeFile(r));
    }) {
        warnings.push(e);
    }
    app.theme_path = Some(theme_path);
    if let Err(e) = upload::prune_uploads(app.uploads_dir()) {
        warnings.push(e);
    }
    #[cfg(windows)]
    if winconsole::classic() {
        warnings.push(
            "this console window shows pictures as blocks; run gramtty in Windows Terminal (wt gramtty)"
                .to_owned(),
        );
    }
    warnings.extend(picker_warning);
    app.startup_warnings(warnings);
    let result = event_loop(&mut terminal, &mut app, &mut rx).await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    let settled = input_barrier().and_then(|()| {
        termprobe::settle_stdio().map_err(|e| format!("cannot drain terminal input: {e}"))
    });
    let mut out = std::io::stdout();
    app.inline.stop();
    let cleared = app
        .inline
        .flush(&mut out)
        .and_then(|()| app.media.anim.shutdown(&mut out))
        .and_then(|()| std::io::Write::flush(&mut out))
        .map_err(|e| format!("terminal: {e}"));
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableFocusChange,
        PopKeyboardEnhancementFlags
    );
    ratatui::restore();
    result.and(settled).and(cleared)
}

fn release_mouse_on_panic() {
    let restore = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        restore(info);
    }));
}

fn hover(ev: &crossterm::event::Event) -> bool {
    matches!(
        ev,
        crossterm::event::Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            ..
        })
    )
}

struct Probed {
    answers: Option<termprobe::Answers>,
    warning: Option<String>,
    image: Option<termprobe::ImageQuery>,
}

fn image_query() -> termprobe::ImageQuery {
    let tmux = Picker::halfblocks().tmux_detected();
    let blocked = ["WEZTERM_EXECUTABLE", "KONSOLE_VERSION"]
        .iter()
        .any(|v| std::env::var(v).is_ok_and(|s| !s.is_empty()));
    termprobe::ImageQuery {
        tmux,
        graphics: !blocked,
    }
}

fn probe_terminal(images: bool) -> Result<Probed, String> {
    let image = images.then(image_query);
    let probe = termprobe::Probe {
        palette: true,
        images: image,
    };
    if crossterm::terminal::enable_raw_mode().is_err() {
        return Ok(Probed {
            answers: None,
            warning: None,
            image,
        });
    }
    let result = termprobe::query_stdio(&probe, termprobe::TIMEOUT);
    let settled = termprobe::settle_stdio();
    crossterm::terminal::disable_raw_mode()
        .map_err(|e| format!("cannot leave raw mode after the terminal query: {e}"))?;
    settled.map_err(|e| format!("cannot drain terminal input: {e}"))?;
    let (answers, warning) = match result {
        Ok(None) => (None, None),
        Ok(Some(o)) => {
            let warning = (!o.complete).then(|| {
                format!(
                    "terminal did not answer its capability query within {} ms; using defaults for what is missing",
                    termprobe::TIMEOUT.as_millis()
                )
            });
            (Some(o.answers), warning)
        }
        Err(e) => (
            None,
            Some(format!("terminal query failed ({e}); using defaults")),
        ),
    };
    termcolors::set(answers.as_ref().and_then(termprobe::Answers::palette));
    Ok(Probed {
        answers,
        warning,
        image,
    })
}

fn input_barrier() -> Result<(), String> {
    crossterm::event::poll(Duration::ZERO)
        .map(|_| ())
        .map_err(|e| format!("terminal input: {e}"))
}

fn requery_palette() -> Result<(Option<termcolors::TermPalette>, usize), String> {
    input_barrier()?;
    let probe = termprobe::Probe {
        palette: true,
        images: None,
    };
    match termprobe::query_stdio(&probe, termprobe::TIMEOUT) {
        Ok(Some(o)) if o.complete => Ok((o.answers.palette(), 0)),
        Ok(Some(o)) => {
            termprobe::settle_stdio().map_err(|e| format!("cannot drain terminal input: {e}"))?;
            let late = if termprobe::owed() == 0 {
                0
            } else {
                probe
                    .string_replies()
                    .saturating_sub(o.answers.string_replies())
            };
            Ok((o.answers.palette(), late))
        }
        Ok(None) => Ok((None, 0)),
        Err(e) => {
            termprobe::settle_stdio().map_err(|e| format!("cannot drain terminal input: {e}"))?;
            Err(format!("terminal colour query failed: {e}"))
        }
    }
}

fn image_picker(probe: Probed) -> (Picker, Option<String>) {
    let forced = std::env::var(format!("{}_IMAGE_PROTOCOL", config::env_prefix())).ok();
    let forced = match forced.as_deref().map(str::trim) {
        None | Some("") | Some("auto") => None,
        Some("halfblocks") => Some(ProtocolType::Halfblocks),
        Some("kitty") => Some(ProtocolType::Kitty),
        Some("sixel") => Some(ProtocolType::Sixel),
        Some("iterm2") => Some(ProtocolType::Iterm2),
        Some(other) => {
            return (
                Picker::halfblocks(),
                Some(format!(
                    "unknown image protocol {other:?}; use auto, kitty, sixel, iterm2 or halfblocks"
                )),
            );
        }
    };
    let image = probe.image.unwrap_or_default();
    let answers = probe.answers.unwrap_or_default();
    let mut picker = match answers.cell.or_else(termprobe::font_size_from_winsize) {
        Some(size) => {
            #[allow(deprecated)]
            let mut p = Picker::from_fontsize(size.into());
            if image.graphics && answers.kitty {
                p.set_protocol_type(ProtocolType::Kitty);
            } else if image.graphics && answers.sixel {
                p.set_protocol_type(ProtocolType::Sixel);
            }
            p
        }
        None => Picker::halfblocks(),
    };
    if let Some(p) = forced {
        picker.set_protocol_type(p);
    }
    (picker, probe.warning)
}

#[cfg(windows)]
mod winconsole {
    use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;

    pub fn owned() -> bool {
        let mut pids = [0u32; 2];
        unsafe { GetConsoleProcessList(pids.as_mut_ptr(), 2) == 1 }
    }

    pub fn classic() -> bool {
        let window = unsafe { GetConsoleWindow() };
        if window.is_null() {
            return false;
        }
        let mut name = [0u16; 64];
        let len = unsafe { GetClassNameW(window, name.as_mut_ptr(), name.len() as i32) };
        String::from_utf16_lossy(&name[..len.max(0) as usize]) == "ConsoleWindowClass"
    }

    pub fn relaunch_in_terminal() -> bool {
        if !classic() || !owned() {
            return false;
        }
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        std::process::Command::new("wt.exe")
            .arg("--")
            .arg(exe)
            .args(std::env::args_os().skip(1))
            .spawn()
            .is_ok()
    }
}

#[cfg(not(unix))]
struct NoSignal;

#[cfg(not(unix))]
impl NoSignal {
    async fn recv(&mut self) -> Option<()> {
        std::future::pending().await
    }
}

async fn event_loop(
    terminal: &mut ratatui::Terminal<
        ui::Resync<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    >,
    app: &mut App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> Result<(), String> {
    let mut input = EventStream::new();
    #[cfg(unix)]
    let mut usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
        .map_err(|e| format!("cannot listen for SIGUSR1: {e}"))?;
    #[cfg(not(unix))]
    let mut usr1 = NoSignal;
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut late = termprobe::LateReplies::default();
    let mut layout = ui::image_layout(&ratatui::buffer::Buffer::empty(terminal.get_frame().area()));
    let mut idle = false;
    loop {
        if !std::mem::take(&mut idle) {
            ui::draw_frame(terminal, app, &mut layout, |_| {})
                .map_err(|e| format!("terminal: {e}"))?;
            app.after_draw();
        }
        if app.exit_ready {
            return Ok(());
        }
        if app.quitting && deadline.is_none() {
            deadline = Some(tokio::time::Instant::now() + Duration::from_secs(5));
        }
        if std::mem::take(&mut app.palette_query) {
            drop(input);
            let palette = requery_palette();
            input = EventStream::new();
            match palette {
                Ok((p, owed)) => {
                    late.expect(owed, std::time::Instant::now());
                    app.palette_answered(p);
                }
                Err(e) => {
                    app.palette_answered(None);
                    app.set_status(e, true);
                }
            }
            continue;
        }
        let tick = app.tick_interval().map(|d| tokio::time::Instant::now() + d);
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(ev)) => {
                    let mut next = Some(ev);
                    idle = true;
                    while let Some(ev) = next {
                        idle &= hover(&ev);
                        if !late.swallow(&ev, std::time::Instant::now()) {
                            app.handle(Event::Input(ev));
                        }
                        next = match crossterm::event::poll(Duration::ZERO) {
                            Ok(true) => Some(
                                crossterm::event::read()
                                    .map_err(|e| format!("terminal input: {e}"))?,
                            ),
                            Ok(false) => None,
                            Err(e) => return Err(format!("terminal input: {e}")),
                        };
                    }
                }
                Some(Err(e)) => return Err(format!("terminal input: {e}")),
                None => return Err("terminal input closed".to_owned()),
            },
            _ = usr1.recv() => app.palette_query = true,
            ev = rx.recv() => match ev {
                Some(ev) => app.handle(ev),
                None => return Err("TDLib receiver stopped".to_owned()),
            },
            _ = tokio::time::sleep_until(tick.unwrap_or_else(tokio::time::Instant::now)), if tick.is_some() => {}
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                return Err("TDLib did not close within 5 s; database may need recovery on next start".to_owned());
            }
        }
        while let Ok(ev) = rx.try_recv() {
            idle = false;
            app.handle(ev);
        }
    }
}
