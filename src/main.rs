mod app;
mod arcade;
mod arcade_state;
mod capture;
mod controller_app;
mod discovery;
mod field;
mod folders;
mod game_controller;
mod launcher;
mod model;
mod navigation;
mod postgame;
mod presentation;
mod rebuild;
mod recent;
mod replay;
mod replay_data;
mod settings;
mod statistics;
mod storage;
mod teams;
mod telemetry;
mod ui;

use std::{env, io, path::PathBuf, time::Duration};

use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let path = args.next();
    if path.as_deref().is_some_and(|s| s == "--help" || s == "-h") {
        println!(
            "robocup-logger [SESSION.json]\nrobocup-logger postgame [SESSION.json]\nrobocup-logger replay [SESSION.json]\nrobocup-logger export SESSION.json\nrobocup-logger rebuild SESSION.json\n\nNo argument: Home menu with New match, Resume, Postgame, Replay, Manage, and Settings.\nReplay loads recorded notes/GC/robots, never live network data; older matches require confirmed field dimensions.\npostgame or replay without a path: choose a session in the built-in picker.\nPass a session JSON directly to resume live logging.\nMenus: arrows / HJKL / WASD, Enter or Space to select, Esc to go back or open actions, ? for help. Tab is unused.\nBindings and new-match defaults are configurable in Settings. Text editors accept literal text.\nLive logging: Enter immediately timestamps a new note; Enter/Esc saves it. Esc when idle opens actions.\nForms: Esc leaves field editing first; leaving changed drafts asks before discarding.\nPostgame: Esc opens the meeting menu; choose Finish to confirm ending the meeting.\nLeaving Home asks for confirmation, defaulting to Cancel.\nManage: regenerate Markdown or rebuild missing GC statistics.\nexport: regenerate .md without changing JSON; close the live logger first.\nrebuild: backfill missing statistics from .gc.jsonl, preserving edits, then export.\nNo microphone recording, transcription, or automatic GitHub submission."
        );
        return Ok(());
    }
    if path
        .as_deref()
        .is_some_and(|s| s == "postgame" || s == "replay")
    {
        let replay = path.as_deref().is_some_and(|s| s == "replay");
        let json = args.next().map(PathBuf::from);
        if args.next().is_some() {
            return Err(io::Error::other(
                "Usage: robocup-logger postgame|replay [SESSION.json]",
            ));
        }
        return interactive(Some(if replay {
            launcher::Launch::Replay(json)
        } else {
            launcher::Launch::Postgame(json)
        }));
    }
    if path
        .as_deref()
        .is_some_and(|s| s == "export" || s == "rebuild")
    {
        let (Some(json), None) = (args.next(), args.next()) else {
            return Err(io::Error::other(
                "Usage: robocup-logger export|rebuild SESSION.json",
            ));
        };
        let json = PathBuf::from(json);
        if path.as_deref().is_some_and(|s| s == "export") {
            println!("Log: {}", storage::Store::export(&json)?.display());
        } else {
            let (store, mut session) = storage::Store::load(&json)?;
            let capture_session = session.capture_session.as_deref().ok_or_else(|| {
                io::Error::other(
                    "This session has no raw capture; missing packets cannot be recovered",
                )
            })?;
            let packets = capture::CaptureLog::read(&store.capture_path(), capture_session)?;
            let changed = rebuild::rebuild(&mut session, &packets).map_err(io::Error::other)?;
            store.save(&session)?;
            println!(
                "Rebuilt {changed} statistics records. Log: {}",
                store.markdown_path().display()
            );
        }
        return Ok(());
    }
    if args.next().is_some() {
        return Err(io::Error::other(
            "Usage: robocup-logger [SESSION.json] | export|rebuild|postgame|replay SESSION.json",
        ));
    }
    interactive(path.map(|path| launcher::Launch::Resume(PathBuf::from(path))))
}

fn interactive(initial: Option<launcher::Launch>) -> io::Result<()> {
    let mut settings = settings::Settings::load()?;
    let home = initial.is_none();
    let mut next = initial;
    let mut terminal = enter_terminal()?;
    let result = (|| {
        loop {
            let launch = match next.take() {
                Some(launch) => launch,
                None => match launcher::run(&mut terminal, &mut settings)? {
                    Some(launch) => launch,
                    None => return Ok(()),
                },
            };
            let workflow = (|| -> io::Result<()> {
                let replay_mode = matches!(&launch, launcher::Launch::Replay(_));
                match launch {
                    launcher::Launch::New
                    | launcher::Launch::NewFromController(_)
                    | launcher::Launch::Resume(_) => {
                        let path = match &launch {
                            launcher::Launch::Resume(path) => Some(path.as_path()),
                            _ => None,
                        };
                        let mut app = app::App::new_with_settings(path, &settings)?;
                        if let launcher::Launch::NewFromController(teams) = launch {
                            app.offer_detected_teams(Some(teams));
                        }
                        run(&mut terminal, &mut app)?;
                    }
                    launcher::Launch::Postgame(path) | launcher::Launch::Replay(path) => {
                        let path = match path {
                            Some(path) => path,
                            None => match launcher::pick_session(&mut terminal, &settings)? {
                                Some(path) => path,
                                None => return Ok(()),
                            },
                        };
                        let (store, mut session) = storage::Store::load(&path)?;
                        if replay_mode
                            && !launcher::confirm_replay_field(
                                &mut terminal,
                                &settings,
                                &store,
                                &mut session,
                            )?
                        {
                            return Ok(());
                        }
                        if let Err(error) = recent::remember(store.session_path()) {
                            launcher::show_error(
                                &mut terminal,
                                &settings.keys,
                                &format!("Cannot update recent sessions: {error}"),
                            )?;
                        }
                        let links = if replay_mode {
                            postgame::run_replay_with_settings(
                                &mut terminal,
                                &store,
                                session,
                                &settings,
                            )?
                        } else {
                            postgame::run_with_settings(&mut terminal, &store, session, &settings)?
                        };
                        if !links.is_empty() {
                            restore_terminal()?;
                            let opened = postgame::open_tabs(&links);
                            terminal = enter_terminal()?;
                            opened?;
                        }
                    }
                }
                Ok(())
            })();
            if !home {
                return workflow;
            }
            if let Err(error) = workflow {
                launcher::show_error(&mut terminal, &settings.keys, &error.to_string())?;
            }
        }
    })();
    let cleanup = restore_terminal();
    result.and(cleanup)
}

fn enter_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    static PANIC_HOOK: std::sync::Once = std::sync::Once::new();
    PANIC_HOOK.call_once(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            previous_hook(info);
        }));
    });
    enable_raw_mode()?;
    let result = (|| {
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        Terminal::new(CrosstermBackend::new(io::stdout()))
    })();
    if result.is_err() {
        let _ = restore_terminal();
    }
    result
}

fn restore_terminal() -> io::Result<()> {
    let raw = disable_raw_mode();
    let screen = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
    raw.and(screen)
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut app::App,
) -> io::Result<()> {
    let mut remembered = false;
    loop {
        app.poll_discovery();
        app.poll_controller();
        app.poll_telemetry();
        if app.save_error.is_none() {
            app.persist();
        }
        if !remembered && let Some(store) = &app.store {
            remembered = true;
            if let Err(error) = recent::remember(store.session_path()) {
                app.status = format!("Cannot update recent sessions: {error}");
            }
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        let refresh = if app
            .controller
            .as_ref()
            .is_some_and(|receiver| !receiver.is_stale())
        {
            50
        } else {
            100
        };
        if !event::poll(Duration::from_millis(refresh))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if app.key(key, chrono::Local::now().fixed_offset()) {
                    return Ok(());
                }
            }
            Event::Paste(text) => app.paste(&text),
            _ => {}
        }
        app.persist();
    }
}
