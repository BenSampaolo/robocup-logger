# RoboCup logger

A local Rust terminal app for timestamp-first match notes. Manual mode needs no server or network connection. Optional GameController listening supplies match events and in-game countdown timestamps. Once listening detects a controller, it automatically requests the undelayed true-data feed. Broadcast-only mode remains available.

## Run

```sh
cd ~/Programming/robocup-logger
cargo run --offline
```

Fill in competition/event, home and away team names, date, match label, robot assignments and output directory. **Match type** defaults to **Golden Goal + official**: one log for the deployment test and official match. Use **←/→** to choose this combined workflow, standalone **Official match**, or standalone **Golden Goal**. The type is fixed after creation. For combined logs, enter the actual official opponent; Golden Goal uses Invisibles without changing those names. Standalone Golden Goal fills in Invisibles and defaults the label to `Golden_Goal`; change the label for another test. Exactly one team must be **HULKs** (case-insensitive); setup shows whether HULKs are home or away. Use **Tab** or **↑/↓** to select **Create match**, then press **Enter**. The default output directory is `logs/` under the directory you launched from. Enter an absolute path to save elsewhere.

New games get a folder named with the match date, local creation hour/minute and label, for example `logs/03-10-2026_14-32-1st_game/`. It contains the dated `Log_03-10-2026_14-32-1st_game.json`, matching `.md`, `.lock`, and (once GC listening starts) `.gc.jsonl` capture. One folder contains Golden Goal plus the official match. Existing flat logs stay where they are and still open/export normally. Creating another match with the same date, minute and label refuses an existing folder rather than overwriting it.

```sh
cargo run --offline -- /path/to/Log_03-10-2026-1st_game.json
```

To launch without Cargo after building: `./target/debug/robocup-logger [SESSION.json]`.

### Regenerate Markdown without the UI

```sh
cargo run --offline -- export /path/to/session.json
# Or, after building:
./target/debug/robocup-logger export /path/to/session.json
```

This regenerates the matching `session.md`, prints its path and exits without a terminal UI or GC connection. The JSON stays byte-for-byte unchanged. The same JSON and renderer version produce identical Markdown, including derived statistics. Close the session in the logger first: export uses the same lock and refuses to overwrite unrecognized Markdown or symlinked files. Existing generated Markdown is replaced; missing Markdown is created.

### Raw capture and rebuilding missing statistics

GC listening automatically creates a matching `.gc.jsonl` sidecar. It saves **complete received packet bytes**, precise wall-clock receipt time, sender, requested feed mode, acceptance/baseline flags and game context. Both broadcast and true-data packets from the selected controller IP are retained, including clock-only and rejected/malformed GC frames. Other controller IPs and pre-selection unrelated traffic are not captured. Capture uses numeric byte arrays, so no packet fields are discarded or dependent on today's decoder. Golden Goal packets are retained for archival purposes but never contribute to official statistics.

Raw batches are saved even while the action menu defers timeline updates. Capture write failures retain the pending batch in memory, show a save error and block quitting; **Tab, w** retries. Capture/session identity and stable packet links prevent accidental mixing. Damaged, truncated, foreign or symlinked captures are rejected without silently deleting data.

```sh
cargo run --offline -- rebuild /path/to/game/Log_game.json
```

Close the logger session first. Unlike `export`, `rebuild` may update JSON: it fills missing GC statistics and inserts missing statistics-only observations from the capture, then exports Markdown. It preserves existing descriptions, timestamps, goal/phase history, corrections, undo state and statistics already present. Running it again does not duplicate observations. It does **not** reconstruct an entire missing goal/phase history: such gaps remain partial coverage rather than silently replacing user decisions. New kinds of derived information can be supported by extending the decoder/rebuilder later; saving raw bytes alone does not make every possible metric available today.

Rebuild leaves the raw capture unchanged. No capture means no backfill; old packets cannot be recovered retroactively. Raw archival records what arrived, not packets lost before reception, robot telemetry, video or manual editing history.

## During a match

| Action | Key |
| --- | --- |
| Capture the moment and start a note while idle | **Enter** |
| Submit the current note and return to idle | **Enter** or **Esc** |
| Capture event time and open the action menu (idle or writing) | **Tab** |
| Return from the action menu without acting | **Esc** |
| Start current game / switch to official after combined test ends (GC off) | **Tab**, then `g` |
| Start Golden Goal (before official stage) | **Tab**, then `t` |
| Halftime / second-half marker (official stage only) | **Tab**, then `h` / `s` |
| Goal for home / away team | **Tab**, then `1` / `2` |
| End the current game (test or official) | **Tab**, then `f` |
| Custom event | **Tab**, then `x` |
| Toggle GameController listening | **Tab**, then `l` |
| Switch between true data and broadcasts | **Tab**, then `v` |
| New note, keeping the unfinished one | **Tab**, then `n` |
| Edit match metadata / robot assignments | **Tab**, then `m` |
| Correct selected event | **Tab**, then `e` |
| Select earlier events while idle | **↑ / ↓** |
| Remove selected event (reversible) | **Tab**, then `d`, then `y` to confirm |
| Restore selected removed event | **Tab**, then `b` |
| Scroll Markdown preview up / down | **Tab**, then `[` / `]` |
| Undo / redo note text | **Tab**, then `u` / `r` |
| Retry saving | **Tab**, then `w` |
| Save and quit | **Tab**, then `q` |

No function keys or Ctrl combinations are required. In the action menu, press a displayed shortcut directly, or use **↑/↓** to highlight an action and **Enter** to execute it. **Tab captures both wall-clock and the latest fresh GC countdown immediately**, shown at the top of the menu. Choosing the event later uses that original time, even after scrolling or moving the highlight. Esc cancels without adding an event; the next Tab captures a fresh time. Menu shortcuts are ordinary characters while typing; they do not trigger actions there.

The menu preserves an unfinished note and its caret position. Markers return you to that note; **Esc** returns without acting. If another observation interrupts your typing, choose **New note** (`Tab`, then `n`): the previous note is kept, and the new note uses the time you pressed Tab. Select the earlier note while idle and use `Tab`, then `e` to finish its description later. Custom events open a description form after immediately saving the marker.

Preview actions keep the menu open so `[` / `]` can be repeated; press **Esc** to resume writing. Other actions return to idle/writing, open a form, or ask for removal confirmation as appropriate.

Notes support cursor movement, Backspace/Delete, Home/End, and undo/redo. Descriptions are single-line; pasted line breaks become spaces. Both of a note's timestamps are captured on Enter before typing and are never advanced by typing or submission. While idle, there is no next-note timestamp running.

### Optional shortcut aliases

The original shortcuts still work; on laptops function keys may require Fn.

| Action | Optional alias |
| --- | --- |
| Open action menu | **F2** |
| Edit match details / correct event | **F3 / F4** |
| Home / away goal | **F5 / F6** |
| Game-start marker | **Ctrl-G** |
| Correct / delete selected event while idle | `e` / `d` or **Delete** |
| Undo / redo text | **Ctrl-Z / Ctrl-Y** |
| Scroll preview, including in forms | **Shift-PgUp / Shift-PgDn** |
| Apply a form; retry saving while idle/writing | **Ctrl-S** |
| Save and quit (finish or cancel an open edit form first) | **Ctrl-C** |

F2–F6 and Ctrl-G act while idle or writing. F2 captures time on opening the menu, just like Tab. Direct goal/start shortcuts capture the timestamp immediately.

## Golden Goal deployment test

The default **Golden Goal + official** session starts with a deployment test against **Invisibles**, with its own score. HULKs keep their configured home/away side; `1` always means home and `2` always means away. Start with **Tab, g** or **Tab, t**, or let GC Playing start it. **Tab, f** or GC Finished ends the test, writes its final score and remaining-message-budget summary, and shows **waiting for official opponent**. Golden Goal has no halftime or second half.

Enable GC listening **during Golden Goal**, so the logger learns and saves the test opponent's number. The combined session switches automatically only **after the test has ended and the opponent number changes on the same GC source**. Restarting with the same opponent does not switch games. With GC listening stopped, **Tab, g** explicitly switches to official after the test ends. The official game then has its own first half, halftime, second half, score and Game End in the same log. Official team names remain those entered in setup; the controller supplies only a numeric opponent ID.

Standalone **Golden Goal** and **Official match** sessions remain available. Standalone Golden Goal accepts **Tab, g** or **Tab, t** and never switches to official; create another session for that game. Game End closes only the current game: further start/goal/phase events and GC imports for it are blocked, while notes and corrections remain usable. Removing or correcting away its Game End reopens that game and requests a fresh GC sync; restoring it closes it again. Live GC telemetry can still update while a game is closed.

## GameController integration

**Tab, l** starts/stops listening on IPv4 UDP **3838**. Only **v20 packets** are supported (`RGme` broadcasts; `RGTD` for automatically requested true data), matching the current branch of the linked [GameController repository](https://github.com/RoboCup-SPL/GameController/blob/7194909358cc6f93fecac93d3a08cbe6c17bd888/src/data/GameControlData.java). That repository now contains HSL support tools; older SPL v18 packets are rejected with a visible notice. The laptop must receive the controller's broadcast network. Bind/receive errors are shown. After a valid broadcast identifies the controller, the logger sends true-data registration requests to it. Broadcast-only mode sends nothing; no discovery probes, robot-return packets or audio are sent.

- HULKs are identified by **team ID 24**, not packet array order, and mapped to the home/away side in setup. Your opponent name labels the other team's score; JSON diagnostics retain its numeric ID.
- The first valid controller source **and opponent ID** are pinned. The combined workflow permits an opponent-ID change from that same source only after Golden Goal ends. Other changes require stopping/restarting listening. The saved test-opponent number survives reopening. If it was never observed, stop listening and use **Tab, g** for the manual transition. A different source cannot trigger a switch while the receiver remains pinned; stop/restart listening if the controller itself restarted. UDP has no authentication: use the trusted match network.
- A persistent red **OPPONENT MISMATCH** warning appears when the configured opponent and received team number disagree. Golden Goal expects Invisibles (0); official matches use the configured opponent. Names are resolved from the bundled GC v7.0.0-rc.3 roster, including `HTWK` → `HTWK Robots` (14), with case-insensitive matching. Unknown names produce an amber **OPPONENT UNVERIFIED** warning, not a guessed match. This is advisory: logging continues, no entries/audio are added, and changing metadata with **Tab, m** clears a resolved mismatch. Custom or newer GC rosters may differ from the bundled roster.
- The GC row shows the **received** score and clock, without extrapolation. Five seconds without accepted packets marks it **STALE**. Goal keys remain locked until listening is stopped; there is no silent manual fallback. Listening is off when reopening a session.
- GC supplies goals and phase transitions for the selected match type. Manual **1/2** (and F5/F6) goals are blocked while listening in new sessions, including Golden Goal, preventing double-counting. Notes, corrections and valid manual phase markers remain available. Repeated phase headings are suppressed. Legacy sessions retain their previous manual-Golden-Goal exception.
- Score, phase/half/state, set-play/kicking-team and stopped/resumed changes are captured in JSON/history. Countdown ticks, packet counters, budget changes and unchanged packets produce **no individual entries**. Official-match player penalty-code changes are also recorded for the end-of-game summary; penalty countdowns and caution-counter ticks do not produce entries. Golden Goal does not collect statistics or log penalty-only changes. The readable export is deliberately smaller (see below).
- Attachment/reconnection records one labelled **sync**, not invented historical goals. A nonzero initial score, a decrease/reset or a multi-goal jump uses one reversible **score adjustment**. A continuous single-goal increment becomes a goal event. Exported score syncs and observed phase markers stay explicitly labelled; a sync with neither a score change nor a phase marker remains diagnostic-only. Normal and extra-time scores contribute to the selected game's score; shootout/timeout diagnostics do not add shootout goals to it.
- Imports preserve the active note/caret and open correction forms. While the action menu holds a captured timestamp, later GC changes queue in memory (the GC row shows the count). Closing the menu applies/saves them after your selected event; quitting also drains the queue. An abrupt process termination with the menu open can lose these pending changes. Removing/restoring imported events changes the **logged** score, while the GC row still shows the external controller's score. Subsequent increments preserve your edits; reconnecting explicitly synchronizes to the newly received score.
- Start a new session for a new official match. The protocol has no match/event ID: a same-opponent reset never triggers the combined test-to-official switch.

### Automatic undelayed true data

Enable listening (**Tab, l**). The first valid controller broadcast automatically triggers a true-data request to that detected IP using `RGTr` version 0 on UDP **3636**; replies arrive as `RGTD` on **3838**. The discovery broadcast is not imported into the log, so its delayed state cannot create an outdated phase or score. No IP is guessed. The status row explicitly shows **GC TRUE WAITING / LIVE / STALE**. Requests retry after silence, not continuously while true packets arrive.

True mode ignores ordinary broadcasts, and broadcast mode ignores true packets. Switching modes establishes a fresh score/phase baseline rather than mixing delayed and undelayed histories. There is no silent fallback if true data is unavailable. Press **Tab, v** for broadcasts (or again for true data), or **Tab, l** to stop listening. Choosing broadcasts stays in effect for that listening connection; restarting listening defaults to automatic true data again. Requests stop when disabled; the protocol has no unsubscribe, so the controller may continue sending packets that the logger ignores. Listening and true-data requests are off when reopening.

### Minimal match notes

Markdown and its live preview show manual notes/events, goals, score corrections/syncs, phase boundaries and end-of-phase budget summaries. Routine GC Ready/Set/Playing, set-play and stop/resume entries are hidden, as are unedited GC snapshot descriptions (team IDs, clocks and repeated state fields). Their complete details remain in JSON and the history/correction UI. Manually edited annotations are preserved verbatim rather than discarded.

New sessions use the GC countdown in Markdown, such as `[09:50]`; it is remaining time, matching the controller display, not elapsed time. Manual captures with no fresh GC reading say `[GC time unavailable]`, never a guessed/extrapolated time. Controller events use the countdown in their own received packet. JSON saves **both** the precise wall-clock `timestamp` and signed `gc_seconds_remaining` (`null` when unavailable).

Existing standalone sessions keep their type; older sessions without `match_type` keep their legacy wall-clock Markdown and phase semantics. Nothing is automatically reclassified or timestamp-reconstructed. Start a new session for the combined workflow. Existing files are not batch-migrated. As with any normal save, reopening an old session can serialize newly supported optional fields as `null`.

### End-of-phase message budgets

A single **HULKs remaining message budget** summary is written at official halftime/full time and at Golden Goal Game End. This is the controller's **remaining allowance**, not a count of messages sent. Automatic endings use their ending packet's budget; manual markers capture the latest fresh reading when **Tab** is pressed, not when an action is chosen. Legacy mixed-session Golden Goal transitions retain their previous pre-transition-budget behavior.

With no fresh GC reading (older than five seconds), the summary says **unavailable**, never a guessed zero. A budget of zero is recorded as zero. Values are saved with their marker, survive reopening/corrections, and disappear/reappear with removal/restore of that marker. Ongoing phases have no final-budget summary yet. Budget-only updates never add log entries.

**Timing:** packets contain no actual goal timestamp. The current controller can delay Playing broadcasts by up to 10 seconds; network/polling adds latency. GC wall-clock timestamps in JSON are **receive times**, alongside the packet's countdown and **GC received** diagnostics. Attachment phase markers are explicitly labelled as observations with unknown original event times, not actual kickoff times. No guessed delay is subtracted. The automatically requested true-data feed avoids the broadcast's deliberate delay; it still supplies snapshots, not original event timestamps. See the [upstream networking notes](https://github.com/RoboCup-HumanoidSoccerLeague/GameController/blob/02b9db9685f8faa7124ef3d464fbf47c1f6bf106/README.md#L66-L90).

## Compact official-match summary

At full time, matches with recorded statistics get two core lines and at most one highlight:

- **Penalties per player:** total observed penalty episodes and how many were incapable (code 5), labelled with the robot assignments. Changing a penalty's reason does not add another episode. Substitution and caution status are not ordinary penalties.
- **Goal share:** `HULKs goals / (HULKs goals + opponent goal kicks + opponent kick-ins) × 100`, with the underlying counts. No counted outcomes means unavailable; zero goals with recorded restarts means 0%. This is a selected-outcome efficiency proxy, **not shooting accuracy**.
- **One optional highlight:** peak observed simultaneous HULKs penalties, if at least two; otherwise a HULKs run of at least three unanswered goals when the goal history is unambiguous. No random selection and no corner-to-goal attribution.

Only official matches are tracked; Golden Goal contributes nothing. Detailed observations stay in JSON/history, not extra readable timeline entries. Removing/restoring observations recalculates the summary; ordinary description corrections preserve their structured data.

These are **observed transitions**, not cumulative counters supplied by the GC. Joining late, reconnecting, detected packet loss or missing historical data marks coverage incomplete. For full coverage, enable listening while the official GC is still **Initial**, with first-half scores at zero: Ready/Set can also occur later and cannot prove pre-kickoff attachment. Already-active penalties on attachment do not invent new penalty starts. Goal share is unavailable when coverage is incomplete, rather than combining a full score with partial restart counts. Ambiguous score corrections suppress the scoring-run highlight. Events that occur entirely between packets cannot be recovered.

Counters follow GC **player numbers**; robot assignments are labels, not automatic hardware-swap detection. Old logs without statistics retain their existing export and cannot be backfilled with player penalties.

## Modes and corrections

A high-contrast banner and matching borders always identify the current mode:

- **Cyan:** idle, ready to mark a moment.
- **Green:** writing a timestamped note.
- **Blue:** setting up a match.
- **Amber:** correcting an event or match details.
- **Magenta:** action menu.
- **Red:** reversible removal confirmation or a save failure.

Mode names are also written out; color is not the only signal. Match phase and score are displayed separately. Use a terminal at least 60 columns × 16 rows; 120 × 35 is more comfortable.

In forms, **Tab/Shift-Tab** or **↑/↓** cycles through the fields and the visible **Apply** (or **Create match**) button. **Enter** advances from a field; when the button is selected, **Enter** applies the form. **Esc** cancels an edit form; during initial setup it exits without creating a match. In an event's type field, **←/→** changes its type, including which team scored. The wall-clock timestamp is explicitly editable in RFC3339 form, e.g. `2026-10-03T14:32:08.123+02:00`. The GC countdown has its own explicit `MM:SS` correction field (negative values supported, blank means unavailable). Text-only corrections preserve both timestamps, including wall-clock submillisecond precision. Event-type choices respect the event's own game, even when correcting Golden Goal after the official match has started.

Scores and half headings are recalculated from the active event history, so correcting/removing/restoring a goal also corrects the score. Events stay in capture order: changing a timestamp does not move an entry between halves. Editing metadata updates the heading, but the output filename stays fixed after creation.

### Remove and restore events

Use **Tab, d**, then `y` to remove the selected note, goal or marker. It remains in the history, dimmed and marked **[undone]**, and stays saved in the JSON session with its original text, type and timestamp. Undone entries are excluded from the generated Markdown and scores. Select one with ↑/↓ while idle and press **Tab, b** to restore it—even after reopening the session. Removing an actively edited note ends editing; restoring it brings back the event, not an active editor.

Event removal/restore is separate from **Tab, u / r**, which undo/redo text edits in the active note. Text-edit history is temporary; removed events remain persisted. GC score adjustments are reversible too: their signed deltas are saved, and displayed scores never go below zero. Changing an adjustment's event type removes its score delta; text-only corrections preserve it.

## Files and timing

The right third of the screen previews the generated Markdown, including unapplied form edits. It follows the bottom automatically; open the action menu with **Tab** and use `[` / `]` to scroll. Scrolling back to the bottom resumes following. The optional Shift-PgUp/PgDn aliases also work inside forms.

Markdown retains the reference log's structure: match heading, robot assignments, half sections, bullet observations, goal lines and scores. New Markdown shows each event's captured GC countdown for match discussion; JSON keeps the full date, precise wall-clock time and timezone offset for robot replay alongside the countdown. Legacy Markdown retains its wall-clock timestamps. This app does **not** synchronize the laptop and robot clocks or control the replay tool.

Each match has three files:

- `.md`: human-readable generated log.
- `.json`: authoritative session data, including match type, each combined entry's game, both timestamps, unfinished notes and all undone events.
- `.lock`: prevents two logger instances from editing the same match. The empty file remains after quitting; its existence does not mean the session is locked.

New notes, typed text and event markers autosave immediately. Form edits apply together when you select **Apply/Create match** and press **Enter**. Saves use atomic file replacement; reopening regenerates the Markdown from JSON if a save was interrupted. Save errors show a red banner and block quitting until a retry succeeds: from idle/writing, press **Tab**, then `w` to retry. Keep the JSON alongside the Markdown for reopening; edit through the app, since the Markdown is regenerated.

## Checks

```sh
cargo test --offline
cargo fmt --check
cargo clippy --offline --all-targets -- -D warnings
```
