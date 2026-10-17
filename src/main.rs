mod app;
mod capture;
mod controller_app;
mod game_controller;
mod model;
mod rebuild;
mod statistics;
mod storage;
mod teams;
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
            "robocup-logger [SESSION.json]\nrobocup-logger export SESSION.json\nrobocup-logger rebuild SESSION.json\n\nNo argument: set up a new match. Pass a session JSON to resume.\nexport: regenerate its .md without the UI or modifying JSON; close the session first.\nrebuild: backfill missing GC statistics from .gc.jsonl, preserving edits, then export.\nSetup defaults to Golden Goal + official in one log; standalone types remain available.\nOfficial follows only after Golden Goal ends and the opponent changes on the same GC source.\nNew Markdown uses GC countdowns; JSON keeps both GC and wall-clock times.\nEnter starts/submits notes. Tab captures both times and opens actions.\nTab then g: start current game; t: test start; f: end current game.\nAfter combined test ends, g starts official manually with GC off.\nTab then l: toggle v20 GC listening (true data requested automatically on detection).\nTab then v: switch between true data and broadcasts.\nTab then d/y removes an event; Tab then b restores it.\nEsc returns from actions. Tab then q saves and quits; Tab then w retries saving.\nForms: select Create/Apply with Tab or arrows, then press Enter.\nFunction-key and Ctrl shortcuts remain optional aliases."
        );
        return Ok(());
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
            "Usage: robocup-logger [SESSION.json] | export|rebuild SESSION.json",
        ));
    }
    let mut app = app::App::new(path.map(PathBuf::from).as_deref())?;
    let mut terminal = enter_terminal()?;
    let result = run(&mut terminal, &mut app);
    let cleanup = restore_terminal();
    result.and(cleanup)?;
    if let Some(store) = &app.store {
        println!(
            "Log: {}\nResume: robocup-logger '{}'",
            store.markdown_path().display(),
            store.session_path().display()
        );
    }
    Ok(())
}

fn enter_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        previous_hook(info);
    }));
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
    loop {
        app.poll_controller();
        if app.save_error.is_none() {
            app.persist();
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if app.key(key, chrono::Local::now().fixed_offset())? {
                    return Ok(());
                }
            }
            Event::Paste(text) => app.paste(&text),
            _ => {}
        }
        app.persist();
    }
}
