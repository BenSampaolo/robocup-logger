# User guide

[← Back to README](../README.md)

## Start and log a match

1. Open **Settings** to set the log folder, event, team/robot defaults, field dimensions, and navigation bindings.
2. Choose **New match**, or **New match from GC** to explicitly choose a tracked team from a detected match.
3. Review the teams, match type, and robot assignments before creating the session.
4. Use **Resume GC logging** in Actions to enable capture/import. Discovery alone does not record a match.
5. Press **Enter** when something happens. The note gets its timestamp immediately; type the observation and press **Enter** or **Esc** to save.

The live view places the event feed, field map, and Markdown preview side by side. The map shows the tracked team's received poses and ball estimates; stale readings disappear. Its attack-up orientation differs from replay's horizontal stadium view.

**Esc** opens Actions when idle. Use the menu to correct or remove events, pause/resume GC logging, reconnect, retry saving, or open the log folder. Removed events remain in the session rather than being erased.

## Replay

Choose **Replay** from Home, or pass a session directly:

```sh
./target/release/robocup-logger replay "path/to/session.json"
```

Replay reads the session and its GC/robot archives—not live devices. It uses a fixed stadium view: teams swap ends at halftime, with the tracked team's direction indicated. New matches save their field dimensions; older sessions require explicit confirmation before replay.

| Key | Replay action |
| :--- | :--- |
| Space | Play/pause; continue past the current note |
| Left / Right | Seek backward/forward five seconds |
| Up / Down | Previous/next human note |
| Home / End | Beginning/end of the timeline |
| + / − | Manual speed, from 0.5× to 16× |
| R | Automatic speed: fast through quiet periods, slower near events |
| 1–5 | Select a tracked-team robot |
| 6–9 / 0 | Select opponent robots 1–5 |
| B | Toggle ball estimates and links |
| H | Toggle opponents |
| PgUp / PgDn | Scroll the current note |
| A / E | Mark an actionable task / edit its details |
| Esc | Meeting actions |

Human notes pause playback, including separate notes at the same timestamp. Robot freshness follows replay time, so pausing does not age poses. Missing archives produce a partial replay with warnings; gaps are not filled with invented movement. GC hundredths are display estimates—the recorded GC countdown has whole-second precision.

## Run a postgame meeting

Use **Postgame** for the note-by-note presentation, or **Replay** for match context. Both use the same saved task details.

- Mark relevant notes as actionable.
- Edit the issue title, robot numbers, volunteer/owner, and discussion or decision.
- Finish without opening tabs, or confirm opening prefilled GitHub issue tabs.

The issue destination is currently `HULKs/hulk`, with the `is:PostgameNotes` and `GO26` labels and the `HULKs/2` project. Safari tab opening requires macOS. Nothing is submitted automatically: review each issue, assign its owner on GitHub, then submit it yourself. Repeating the finish action can reopen the same issues.

Issue titles and bodies travel in the GitHub URLs. Review their contents before opening tabs; do not include secrets or sensitive material. There is no microphone recording, transcription, or transcript viewer.

## Data & networking

The default log root is `logs/`; change it in Settings. Keep a session and its companion archives together:

| File | Purpose |
| :--- | :--- |
| `*.json` | Authoritative session, notes, corrections, task details, and replay context |
| `*.md` | Generated, readable match report |
| `*.gc.jsonl` | Raw captured GameController datagrams and receipt timestamps |
| `*.robots.jsonl` | Full robot datagrams forwarded by the selected GC, including both teams and undecodable payloads |
| `*.lock` | Session lock file; its presence alone does not mean a logger is running |

Settings live at `$XDG_CONFIG_HOME/robocup-logger/settings.json`, or `~/.config/robocup-logger/settings.json` when that variable is unset. Recent-session history is stored alongside them.

- **UDP 3838:** receive GameController data.
- **UDP 3636:** send GC monitor requests. Discovery tries `10.0.0.10` and local broadcasts; macOS also uses interface-specific broadcasts.
- **UDP 3940:** receive robot messages forwarded by the selected GC. The logger sends no robot messages.

Forwarded telemetry uses the HSL `RGrt` v4 format with the original sender's IPv4 prefix. Forwarding depends on GC monitor registration; broadcast-only mode does not maintain that registration. Other GC variants may not supply compatible telemetry.

Archives contain packets received by the logger, not an assurance of lossless UDP delivery. On a save failure, capture pauses while retaining already captured unsaved data. Retry saving from Actions; recovery starts a fresh baseline rather than treating queued old packets as current. Reachability alone does not prove that the selected match's events are being imported.

## Export and recovery

Close the live logger for a session before using these commands:

```sh
./target/release/robocup-logger export "path/to/session.json"
./target/release/robocup-logger rebuild "path/to/session.json"
```

**Export** regenerates Markdown without changing JSON. **Rebuild** fills missing statistics from the GC archive, preserves existing corrections, and saves the result. Both are also available under **Manage**.

Keep backups of JSON and capture archives. Markdown is generated output, not the place to make persistent corrections. Existing recordings or transcripts from older versions are not deleted, but this version does not open or create them.
