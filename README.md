# RoboCup logger

A local Rust terminal app for timestamp-first match notes. Manual logging needs no server or network connection. Automatic GameController discovery shows network reachability without enabling logging. Optional GameController logging supplies match events and in-game countdown timestamps using the automatically requested undelayed true-data feed. Broadcast-only logging remains available.

## Run

```sh
cd ~/Programming/robocup-logger
cargo run --offline
```

The **Home** menu offers **New match**, **Resume match**, **Postgame meeting**, **Manage sessions / recordings**, **Settings**, and **Exit**. Resume and Postgame use the built-in session picker: select folders or JSON files; `../` goes to the parent folder and Back cancels. Yazi is not required. The UI keeps controls, status/errors and safety confirmations visible; explanatory guidance lives in this README. Footers occupy one line, with status/errors prioritized. Control hints show each command's primary binding; configured aliases still work. Session pickers and team dropdowns support **/** substring search: type a name, Enter finishes the filter, then navigate and confirm a result. Esc clears the filter before closing the list; `../` remains available in the picker. Search is disabled if `/` is assigned to a navigation command. Returning from a workflow launched through Home returns to Home; direct CLI paths still exit after that workflow. Leaving Home via **Esc**, **Ctrl-C**, or **Exit** asks **Are you sure you want to quit?**, defaulting to **Cancel**. Esc or confirming Cancel returns to Home. Up to eight recently created/opened sessions appear below the main actions; select one to choose Resume or Postgame, with Cancel initially selected. Recent paths live in `recent_sessions.json` beside Settings, separately from match data. Missing files are omitted; malformed recent-state files are reported rather than overwritten.

Menus use **arrow keys / HJKL / WASD**, **Enter or Space** to select, **Esc** to go back or open Actions, and **?** for help. **Tab and Shift-Tab are unused**, including in text editors. Menu selection wraps from the last item to the first and vice versa, including dropdowns and confirmations; note/history navigation and text scrolling do not wrap. Footer hints reflect your configured bindings. Letter navigation applies only outside text editing. Select a field to edit it; letters and spaces then remain literal text. Actions and confirmations appear as focused pop-ups over dimmed context; recording, controller, save, and error status stays visible. Former per-action letter and function-key shortcuts are replaced by visible menus. **Ctrl-C** requests a safe exit rather than bypassing saving/recording cleanup.

**Settings** contains the log folder, Python interpreter and Whisper cache paths, default event/tracked team/opponent, five robot assignments, and navigation keybindings. Edits remain a draft until **Save settings**. **Enter/Esc** in a text field keeps that field's draft and returns to selection. Leaving changed Settings or keybindings asks before discarding; the confirmation defaults to **Cancel**, and Esc keeps the draft. Unchanged drafts go back immediately. Defaults apply only to new matches; existing sessions retain their own metadata. Key aliases are space-separated names such as `Up k w`, `Enter Space`, or `Ctrl+x`; every command needs a binding, conflicts and reserved Ctrl-C are rejected. Changes take effect after saving. The retired Menu binding is accepted when reading older settings but ignored and omitted on the next save; Tab cannot be rebound. In text editors, Enter/Esc have explicit text-editing semantics rather than character navigation. Settings are stored in `$XDG_CONFIG_HOME/robocup-logger/settings.json`, or `~/.config/robocup-logger/settings.json`; missing settings use defaults without creating a file. Invalid files are reported, not silently reset or overwritten.

**Manage** selects a session, then offers Markdown regeneration, confirmed statistics rebuild, or browsing local meeting recordings/transcripts. No deletion or renaming is provided. Neither browsing nor settings activates the microphone or downloads models.

Choose **New match**, then fill in competition/event, date, match label, robot assignments and output directory, and choose **Tracked team** and **Opponent** from the dropdowns. **Match type** defaults to **Golden Goal + official**: one log for the deployment test and official match. Use **←/→** to choose this combined workflow, standalone **Official match**, or standalone **Golden Goal**. The type is fixed after creation. For combined logs, enter the actual official opponent; Golden Goal uses Invisibles without changing those names. Standalone Golden Goal fills in Invisibles and defaults the label to `Golden_Goal`; change the label for another test. The dropdowns contain exactly the **66 teams in GameController v7.0.0-rc.3** (`config/teams.yaml`); arbitrary team names and selecting the same team twice are rejected. Confirm opens a team list, arrows/shared navigation move the selection, Confirm chooses, and Esc cancels. Robot assignments, penalties, budget and goal-share statistics follow the tracked team. New matches place it on the home side; existing logs retain their original home/away perspective. Invisibles can be tracked in Official matches, but not in Golden Goal or combined matches, where it is already the test opponent. Navigate fields with the shared controls, select a text field to edit, then select **Create match** and confirm. The setup-only **Use detected GC teams** button offers the two currently detected roster teams. Explicitly choose which to track; the confirmation labels state that this fills both draft teams and selects **Official match**. Cancel changes nothing, and you can change the match type before creating the log. No received packet silently changes a draft or existing match. The default output directory is `logs/` under the directory you launched from. Enter an absolute path to save elsewhere.

New games get a folder named with the match date, local creation hour/minute and label, for example `logs/03-10-2026_14-32-1st_game/`. It contains the dated `Log_03-10-2026_14-32-1st_game.json`, matching `.md`, `.lock`, and (once GC listening starts) `.gc.jsonl` capture. One folder contains Golden Goal plus the official match. Existing flat logs stay where they are and still open/export normally. Creating another match with the same date, minute and label refuses an existing folder rather than overwriting it.

```sh
cargo run --offline -- /path/to/Log_03-10-2026-1st_game.json
```

To launch without Cargo after building: `./target/debug/robocup-logger [SESSION.json]`.

### Postgame meeting

```sh
cargo run --offline -- postgame                      # session picker
cargo run --offline -- postgame /path/to/SESSION.json # direct path
```

Close the live logger first: postgame uses the same session lock. The terminal presents active notes/events in capture order, with wall-clock and GC timestamps, and an upcoming-note list. Removed events and unannotated GC diagnostic/score-sync entries are omitted, including syncs that adjust the score. Goals, phase events, penalty changes and human annotations remain available. Generated GC entries use short event/score or penalty summaries in the presentation and live history; full stored diagnostics are unchanged. Use a terminal at least **60 × 20**.

- The left side shows a dimmed **Previous** note above the **current note in bold yellow**, and a dimmed **Next** note below. Action items have a green label. All three notes show muted wall-clock times and GC countdowns (`—` when unavailable). The right side keeps the full agenda, automatically scrolling with the selection. Detailed context is omitted from presentation; robots, volunteer, and discussion remain available through **Edit details**.
- Shared **Up/Down** controls navigate notes, including skipping or revisiting them without deleting anything. **PgUp/PgDn** scroll the current note.
- **Esc** from presentation opens the meeting actions pop-up: **Mark/Unmark task**, **Edit details**, **View transcript**, **Retry transcription**, and either finish option. **Enter** and **Space** remain aliases. **Esc** inside the menu closes it without ending the meeting or stopping recording. Choose a finish option to request its confirmation; **Esc** or **Cancel** cancels that confirmation.
- **Edit details** contains the issue title, robots, volunteer, and discussion/decision. Select a field to type; **Enter** adds a newline and **Esc** returns to field selection, retaining the draft. Choose **Apply and save** to save. **Esc** from field selection or **Cancel changes** leaves unchanged details immediately; changed details require a discard confirmation, defaulting to **Cancel**. Editing details alone does not mark the note.
- **Finish + GitHub tabs** opens one prefilled new-issue tab in **Safari** per marked note after confirmation. **Finish without tabs** saves/exits without opening tabs. Both confirmations default to **Cancel**.

Marks and applied details are saved alongside their original entries in JSON and survive reopening, correction, and rebuild. Match notes, scores, timestamps and generated match Markdown remain unchanged. Save failures block finishing/exiting; choosing finish again retries saving. Reopening and finishing again opens the marked tabs again; the logger does not track whether you submitted an issue.

The postgame Actions menu also offers **Jump to note**: type note text or its displayed agenda number, use Up/Down to choose a result, and Enter to jump. Esc cancels. Jumps use the same recording markers as ordinary navigation. The last presented note is saved with the session and restored on reopening (or the nearest visible note if it was hidden/removed); recording still requires consent every time.

**Open log folder** is available in live/Postgame Actions and Manage. It opens the session folder in Finder on macOS (the default file manager on Linux), without opening the recording itself.

Links target `HULKs/hulk`, with title prefix `post-game task:`, type `task`, labels `is:PostgameNotes,GO26`, and project `HULKs/2`. The body includes the original note, precise wall-clock timestamp, GC countdown when available, match context, robot assignments, selected robots, volunteer, and decisions. The volunteer is text in the body, not an automatic GitHub assignee. Review/edit and submit on GitHub yourself; no issues are submitted by the logger. GitHub applies labels/project/type according to its URL support and your permissions. Opening the tabs sends their prefilled contents to GitHub and may retain them in browser history; review the marked notes before finishing. Very large notes may exceed browser/server URL limits.

#### Optional recording and local German transcription

- At the **meeting-start screen**, the default is **Continue without recording**. Select **Everyone consented—record meeting** only after everyone has agreed, then confirm. **Back / exit** leaves before starting. This choice is made once per meeting; there is no per-note recording toggle. To review an existing transcript, continue without recording.
- Wait for the red **REC** indicator before presenting; it appears only after audio has actually been written. macOS may separately request microphone permission; note navigation is blocked until recording is ready. Back requests ending/cancelling startup. Recording never starts automatically, and hidden confirmations are disabled in undersized terminals.
- Present normally: every note switch, skip, and revisit logs an audio-time marker automatically. The microphone keeps running through navigation and editing.
- Confirm either **Finish** option to stop recording, transcribe locally, and group discussion by note/task using the logged presentation intervals. The full audio stays intact; no audio clips are cut. While waiting to exit, Back asks whether to cancel pending/running transcription and return to Postgame, defaulting to **Keep waiting**. Esc dismisses that confirmation and continues waiting. Only confirming cancellation stops the work; audio/timings remain saved. You can retry or finish without a transcript. Recording is not restarted.
- **View transcript** opens a dedicated reader for the latest started recording. The original note is highlighted **yellow**, spoken discussion is **white** with paragraph spacing, and context is muted. A topic list lets you move between notes with **Up/Down**. The note is shown once, followed by all discussion in chronological order, with a muted “Later in the discussion” separator between visits instead of timestamps and repeated headings. If the note text changed, the updated snapshot appears at that point. **Left/Right** or **PgUp/PgDn** scroll through them; **Home/End** jumps to the beginning/end; **Esc/Back** returns. Configured directional bindings also work. The same viewer is available through **Manage → meeting recordings**. Reading does not move the live presentation, add recording markers, or modify transcript/Markdown/audio files. It requires the saved structured transcript and timeline, not just a Markdown copy.
- **Retry transcription** processes the latest started recording, including after reopening. Failed/denied startup attempts do not replace the previous recording in this view.

The recorder selects the MacBook's **built-in microphone**, never an external/default-device fallback. It does not change system input settings. Startup can retry a stopped engine briefly before any audio is captured, only with the same microphone and format. Once audio has begun, engine interruptions, input changes, or dropped frames stop recording rather than hide gaps in the timeline. A native helper is compiled locally using installed Apple command-line tools/Swift on the first recording request in each app run; ordinary logging does not require Swift. No GameController connection is started in postgame.

By default, transcription reuses the existing `~/Programming/phillip-neutts-german/.venv/bin/python` environment and its cached `models/hub/models--Systran--faster-whisper-medium` model. The interpreter and model-cache folder can be changed in **Settings**; the configured environment must already contain Faster-Whisper, NumPy and SoundFile, and the cache must contain the medium model. It targets **German**, with English technical terms potentially needing correction. It uses CPU/int8 inference after the meeting, not live recognition. Installed **FFmpeg** decodes audio, bypassing a PyAV compatibility problem in that shared environment. Nothing in that project is edited or installed. If the environment/model is missing, transcription fails clearly; it does not download anything. Inference runs under macOS `sandbox-exec` with **network access denied**, using a local model path and offline mode. There is no cloud fallback.

Each recording is retained in a private subdirectory of `SESSION.postgame/` next to `SESSION.json`:

- `audio.caf`: uncompressed microphone recording.
- `metadata.json`, `recording.started`, `timeline.jsonl`: meeting context, successful startup, and audio-time markers with immutable snapshots of the notes being presented.
- `transcript.json`: recognized words and audio timestamps.
- `transcript.md`: readable speech grouped by presented note/task, including original note text, wall-clock timestamps and GC countdowns. Revisited notes have multiple chronological presentation windows under the same note; words spanning a switch are identified as overlapping. All presented notes are included, not just those marked for GitHub issues.

**Timing is approximate:** note boundaries use delivered audio-buffer progress (typically 100–400 ms granularity), and recognized word times are estimates, not exact speaker/word boundaries. Audio is retained for verification. Speech crossing a presentation boundary can appear in adjacent visits; the viewer preserves this overlap rather than deleting spoken text. Editing/rebuilding the match later does not rewrite the recording's note snapshots. Separate meetings remain separate recordings; postgame transcript actions target the latest started one. **Manage** also lets you browse older recordings/transcripts.

Transcription failures or cancellation preserve the audio and timeline for retry. Damaged timelines are rejected rather than silently guessing note associations; a truncated final marker after a crash may require manual recovery. Transcripts/audio are never added to GitHub links automatically. Files are local, but any backup/sync software you run may still copy the containing folder.

### Regenerate Markdown without the UI

```sh
cargo run --offline -- export /path/to/session.json
# Or, after building:
./target/debug/robocup-logger export /path/to/session.json
```

This regenerates the matching `session.md`, prints its path and exits without a terminal UI or GC connection. The JSON stays byte-for-byte unchanged. The same JSON and renderer version produce identical Markdown, including derived statistics. Close the session in the logger first: export uses the same lock and refuses to overwrite unrecognized Markdown or symlinked files. Existing generated Markdown is replaced; missing Markdown is created.

### Raw capture and rebuilding missing statistics

GC listening automatically creates a matching `.gc.jsonl` sidecar. It saves **complete received packet bytes**, precise wall-clock receipt time, sender, requested feed mode, acceptance/baseline flags and game context. Both broadcast and true-data packets from the selected controller IP are retained, including clock-only and rejected/malformed GC frames. Other controller IPs and pre-selection unrelated traffic are not captured. Capture uses numeric byte arrays, so no packet fields are discarded or dependent on today's decoder. Golden Goal packets are retained for archival purposes but never contribute to official statistics.

Raw batches are saved even while the action menu defers timeline updates. Capture write failures retain the pending batch in memory, show a save error and block quitting; choose **Retry saving** in the actions menu to retry. Capture/session identity and stable packet links prevent accidental mixing. Damaged, truncated, foreign or symlinked captures are rejected without silently deleting data.

```sh
cargo run --offline -- rebuild /path/to/game/Log_game.json
```

Close the logger session first. Unlike `export`, `rebuild` may update JSON: it fills missing GC statistics and inserts missing statistics-only observations from the capture, then exports Markdown. It preserves existing descriptions, timestamps, goal/phase history, corrections, undo state and statistics already present. Running it again does not duplicate observations. It does **not** reconstruct an entire missing goal/phase history: such gaps remain partial coverage rather than silently replacing user decisions. New kinds of derived information can be supported by extending the decoder/rebuilder later; saving raw bytes alone does not make every possible metric available today.

Rebuild leaves the raw capture unchanged. No capture means no backfill; old packets cannot be recovered retroactively. Raw archival records what arrived, not packets lost before reception, robot telemetry, video or manual editing history.

## During a match

| Action | Default control |
| --- | --- |
| Capture the moment and start a note while idle | **Enter** |
| Submit the current note and return to idle | **Enter** or **Esc** |
| Capture event time and open the actions pop-up | **Esc** when idle; while typing, **Esc** saves the note, then **Esc** opens Actions |
| Select an action | **↑/↓**, **J/K**, or **W/S**, then **Enter/Space** |
| Return without acting | **Esc** |
| Select earlier events while idle | Shared navigation controls |
| Undo / redo text while editing | **Ctrl-Z / Ctrl-Y** |
| Scroll Markdown preview, including in forms | **Shift-PgUp / Shift-PgDn** |
| Save and quit safely | Actions → **Save and quit**, or **Ctrl-C** |

The actions menu contains game-start/end, Golden Goal, halftime, second-half, home/away goals, custom events, new notes, metadata and event corrections, removal/restoration, GC listening/feed mode, preview scrolling, save retry, and text undo/redo. No function keys or Ctrl combinations are required. Removal confirmation defaults to **Cancel**.

**Opening the actions menu captures both wall-clock and the latest fresh GC countdown immediately**, shown at the top. Choosing an event later uses that original time, even after scrolling or moving the highlight. Back cancels without adding an event; reopening captures a fresh time. Navigation letters are ordinary characters while typing; they do not trigger actions there. Immediate **Enter** timestamping remains fixed even with customized menu bindings.

The menu preserves an unfinished note and its caret position. Markers return you to that note. If another observation interrupts typing, choose **New note (keep unfinished draft)**: the previous note is kept, and the new note uses the time you opened the menu. Select an earlier note while idle and choose **Correct selected event** to finish its description later. Custom events open a description form after immediately saving the marker.

Preview actions keep the menu open for repeated scrolling; Back resumes writing. Other actions return to idle/writing, open a form, or ask for removal confirmation.

Notes support cursor movement, Backspace/Delete, Home/End, and undo/redo. Descriptions are single-line; pasted line breaks become spaces. Both timestamps are captured on Enter before typing and never advanced by typing or submission. While idle, there is no next-note timestamp running.

## Golden Goal deployment test

The default **Golden Goal + official** session starts with a deployment test against **Invisibles**, with its own score. The tracked team keeps its configured home/away side; goal actions explicitly name the side. Choose **Game Start** or **Golden Goal Start** from the actions menu, or let GC Playing start it. **Game End** or GC Finished ends the test, writes its final score and remaining-message-budget summary, and shows **waiting for official restart**. Golden Goal has no halftime or second half.

Enable GC listening **during Golden Goal**, so the logger learns and saves the test opponent's number. The combined session switches automatically **after the test has ended**, either when the opponent number changes on the selected GC source, or when that source reports a **same-opponent reset: Normal phase, first-half Initial/Ready/Set, both scores zero**. A stale/recreated UDP sender or repeated Finished/Playing test packets alone do not switch games. With GC listening stopped, **Game Start** explicitly switches to official after the test ends. The official game then has its own first half, halftime, second half, score and Game End in the same log. Official team names remain those entered in setup; the controller supplies only a numeric opponent ID.

Standalone **Golden Goal** and **Official match** sessions remain available. Standalone Golden Goal accepts **Game Start** or **Golden Goal Start** and never switches to official; create another session for that game. Game End closes only the current game: further start/goal/phase events and GC imports for it are blocked, while notes and corrections remain usable. Removing or correcting away its Game End reopens that game and requests a fresh GC sync; restoring it closes it again. Live GC telemetry can still update while a game is closed.

## GameController integration

**Home**, new-match setup and the live screen automatically discover an active GC while logging is off. Discovery sends `RGTr` version 0 requests to UDP **3636** at the expected address **10.0.0.10** and LAN broadcast addresses; `RGTD` replies arrive on UDP **3838**. A validated packet, not the expected IP or a successful send alone, establishes reachability; the status stays stable across request replies and broadcasts, changing only when connectivity, controller address or teams change. Requests are rate-limited to once every two seconds while searching, and detection becomes stale after five seconds without valid data. On macOS, LAN requests include each interface's IPv4 broadcast address as well as the limited broadcast address, so a VPN's default route need not hide the attached LAN. The status shows the detected controller and team IDs; **no reply is inconclusive**, not proof that the GC is down. UDP bind conflicts and receive errors are visible. Discovery only observes: it does not create a session, import match events or disable manual goals.

Actions → **Pause GC logging** / **Resume GC logging** pauses/resumes importing into this log; it does not stop the GameController. Logging listens on IPv4 UDP **3838**. Discovery releases its socket before logging starts. Only **v20 packets** are supported (`RGme` broadcasts; `RGTD` for automatically requested true data), matching [GC v7.0.0-rc.3](https://github.com/RoboCup-HumanoidSoccerLeague/GameController/blob/v7.0.0-rc.3/game_controller_msgs/headers/RoboCupGameControlData.h); older SPL v18 packets are rejected with a visible notice. The laptop must have a network route to the controller; broadcasts require the same broadcast network. Bind/receive errors are shown. Broadcast-only logging sends no requests. No robot-return packets or audio are sent by GC discovery or logging.

- The selected **tracked team ID**, not a hard-coded HULKs ID or packet array position, determines whose scores, penalties and budgets are imported. Broadcasts for other teams show a detection notice but are not imported. Your opponent selection labels the other team's score; JSON diagnostics retain the received opponent ID. Old logs without an explicit tracked-team ID retain their HULKs (24) perspective.
- The first valid controller source **and opponent ID** are pinned. The combined workflow permits an opponent-ID change from that same source only after Golden Goal ends. The saved test and official opponent numbers survive reopening, including same-opponent handovers. If the test opponent was never observed, stop listening and choose **Game Start** for the manual transition. Other IPs cannot take over automatically. **Actions → Reconnect/switch GC (same match)** opens a Cancel-default confirmation: reconnect flushes/saves pending events, preserves history and capture ownership, resets source pinning, and reacquires the current accepted opponent (or the configured opponent when not yet observed). Save errors block the reset. The first matching controller wins; this may select the same controller again. It does not edit team names or start another match. UDP has no authentication: use the trusted match network.
- A persistent red **OPPONENT MISMATCH** warning appears when the configured opponent and received team number disagree. Golden Goal expects Invisibles (0); official matches use the configured opponent. Names are resolved from the bundled GC v7.0.0-rc.3 roster, including `HTWK` → `HTWK Robots` (14), with case-insensitive matching. Unknown names produce an amber **OPPONENT UNVERIFIED** warning, not a guessed match. This is advisory: logging continues, no entries/audio are added, and changing metadata with **Edit match details** clears a resolved mismatch. New selections are restricted to the bundled roster; unknown legacy labels remain readable. Custom or newer GC rosters require a roster update.
- The GC row shows the **received** score and clock, without extrapolation. Five seconds without accepted packets marks it **STALE**. Goal actions remain locked until listening is stopped; there is no silent manual fallback. Listening is off when reopening a session.
- GC supplies goals and phase transitions for the selected match type. Manual home/away goal actions are blocked while listening in new sessions, including Golden Goal, preventing double-counting. Notes, corrections and valid manual phase markers remain available. Repeated phase headings are suppressed. Legacy sessions retain their previous manual-Golden-Goal exception.
- Score, phase/half/state, set-play/kicking-team and stopped/resumed changes are captured in JSON/history. Countdown ticks, packet counters, budget changes and unchanged packets produce **no individual entries**. Official-match player penalty-code changes are also recorded for the end-of-game summary; penalty countdowns and caution-counter ticks do not produce entries. Golden Goal does not collect statistics or log penalty-only changes. The readable export is deliberately smaller (see below).
- Attachment/reconnection records one labelled **sync**, not invented historical goals. A nonzero initial score, a decrease/reset or a multi-goal jump uses one reversible **score adjustment**. A continuous single-goal increment becomes a goal event. Exported score syncs and observed phase markers stay explicitly labelled; a sync with neither a score change nor a phase marker remains diagnostic-only. Normal and extra-time scores contribute to the selected game's score; shootout/timeout diagnostics do not add shootout goals to it.
- Imports preserve the active note/caret and open correction forms. While the action menu holds a captured timestamp, later GC changes queue in memory (the GC row shows the count). Closing the menu applies/saves them after your selected event; quitting also drains the queue. An abrupt process termination with the menu open can lose these pending changes. Removing/restoring imported events changes the **logged** score, while the GC row still shows the external controller's score. Subsequent increments preserve your edits; reconnecting explicitly synchronizes to the newly received score.
- Start a new session for a new official match. The protocol has no match/event ID: same-opponent handover uses the reset evidence above, not UDP port changes. Late Finished snapshots are ignored while that new official feed is still pregame; observing Playing releases this guard. If Playing was missed, explicit reconnect clears it and rebaselines. Source/sequence checks reject old senders and out-of-order packets, but cannot distinguish old test state from legitimate official state sent by the selected feed with a valid sequence. Multiple live GCs for the same teams cannot be reliably distinguished.

### Automatic undelayed true data

Enable match logging with **Resume GC logging**. The receiver actively requests true data using `RGTr` version 0 on UDP **3636**; replies arrive as `RGTD` on **3838**. A valid reply containing the tracked team can establish the controller immediately, without waiting for a broadcast. A received broadcast can also discover its address and trigger a directed request. The discovery broadcast is not imported into the log, so its delayed state cannot create an outdated phase or score. Probing the expected IP does not assume it is a GC: only validated responses establish detection. The status row separates transport (**waiting / live / stale feed**) from imports: **OFF / NOT IMPORTING** even when discovery can reach a GC, **WAITING OFFICIAL** after the combined test ends, **FINISHED** after a completed match, **IMPORT QUEUED** while Actions defers updates, and **IMPORTING** for an active accepted feed. Requests retry after silence, not continuously while true packets arrive.

True mode ignores ordinary broadcasts, and broadcast mode ignores true packets. Switching modes establishes a fresh score/phase baseline rather than mixing delayed and undelayed histories. There is no silent fallback if true data is unavailable. Use the GC feed-mode action to switch to broadcasts (or back to true data), or **Pause GC logging** to pause match logging. Choosing broadcasts stays in effect for that logging connection; restarting logging defaults to automatic true data again. Match import is off when reopening, but discovery resumes automatically while logging is off. The protocol has no unsubscribe, so the controller may continue sending replies after a request stops. Discovery never silently enables match import.

### Minimal match notes

Markdown and its live preview show manual notes/events, goals, score corrections/syncs, phase boundaries and end-of-phase budget summaries. Routine GC Ready/Set/Playing, set-play and stop/resume entries are hidden, as are unedited GC snapshot descriptions (team IDs, clocks and repeated state fields). Their complete details remain in JSON and the history/correction UI. Manually edited annotations are preserved verbatim rather than discarded.

New sessions use the GC countdown in Markdown, such as `[09:50]`; it is remaining time, matching the controller display, not elapsed time. Manual captures with no fresh GC reading say `[GC time unavailable]`, never a guessed/extrapolated time. Controller events use the countdown in their own received packet. JSON saves **both** the precise wall-clock `timestamp` and signed `gc_seconds_remaining` (`null` when unavailable).

Existing standalone sessions keep their type; older sessions without `match_type` keep their legacy wall-clock Markdown and phase semantics. Nothing is automatically reclassified or timestamp-reconstructed. Start a new session for the combined workflow. Existing files are not batch-migrated. As with any normal save, reopening an old session can serialize newly supported optional fields as `null`.

### End-of-phase message budgets

A single **tracked-team remaining message budget** summary is written at official halftime/full time and at Golden Goal Game End. This is the controller's **remaining allowance**, not a count of messages sent. Automatic endings use their ending packet's budget; manual markers capture the latest fresh reading when the actions menu opens, not when an action is chosen. Legacy mixed-session Golden Goal transitions retain their previous pre-transition-budget behavior.

With no fresh GC reading (older than five seconds), the summary says **unavailable**, never a guessed zero. A budget of zero is recorded as zero. Values are saved with their marker, survive reopening/corrections, and disappear/reappear with removal/restore of that marker. Ongoing phases have no final-budget summary yet. Budget-only updates never add log entries.

**Timing:** packets contain no actual goal timestamp. The current controller can delay Playing broadcasts by up to 10 seconds; network/polling adds latency. GC wall-clock timestamps in JSON are **receive times**, alongside the packet's countdown and **GC received** diagnostics. Attachment phase markers are explicitly labelled as observations with unknown original event times, not actual kickoff times. No guessed delay is subtracted. The automatically requested true-data feed avoids the broadcast's deliberate delay; it still supplies snapshots, not original event timestamps. See the [upstream networking notes](https://github.com/RoboCup-HumanoidSoccerLeague/GameController/blob/02b9db9685f8faa7124ef3d464fbf47c1f6bf106/README.md#L66-L90).

## Compact official-match summary

At full time, matches with recorded statistics get two core lines and at most one highlight:

- **Penalties per player:** total observed penalty episodes and how many were incapable (code 5), labelled with the robot assignments. Changing a penalty's reason does not add another episode. Substitution and caution status are not ordinary penalties.
- **Goal share:** `tracked-team goals / (tracked-team goals + opponent goal kicks + opponent kick-ins) × 100`, with the underlying counts. No counted outcomes means unavailable; zero goals with recorded restarts means 0%. This is a selected-outcome efficiency proxy, **not shooting accuracy**.
- **One optional highlight:** peak observed simultaneous tracked-team penalties, if at least two; otherwise a tracked-team run of at least three unanswered goals when the goal history is unambiguous. No random selection and no corner-to-goal attribution.

Only official matches are tracked; Golden Goal contributes nothing. Detailed observations stay in JSON/history, not extra readable timeline entries. Removing/restoring observations recalculates the summary; ordinary description corrections preserve their structured data.

These are **observed transitions**, not cumulative counters supplied by the GC. Joining late, reconnecting, detected packet loss or missing historical data marks coverage incomplete. For full coverage, enable listening while the official GC is still **Initial**, with first-half scores at zero: Ready/Set can also occur later and cannot prove pre-kickoff attachment. Already-active penalties on attachment do not invent new penalty starts. Goal share is unavailable when coverage is incomplete, rather than combining a full score with partial restart counts. Ambiguous score corrections suppress the scoring-run highlight. Events that occur entirely between packets cannot be recovered.

Counters follow GC **player numbers**; robot assignments are labels, not automatic hardware-swap detection. Old logs without statistics retain their existing export and cannot be backfilled with player penalties.

## Modes and corrections

High-contrast labels and a shared color hierarchy make the active content clear:

- **Bright yellow:** current note/input and cautions.
- **Cyan:** focused controls and selected actions, with contrasting highlights.
- **Green:** positive status and marked tasks.
- **Red:** errors, active microphone recording, and destructive controls.
- **Muted gray:** secondary context and the backdrop behind a pop-up.

Panels use solid colored frames—one full character row across the top/bottom and two columns down each side—with contrasting titles and inner padding. Roomy layouts also leave space above and below the text. Pop-ups grow to add padding without reducing room for their controls, and small layouts stay compact. Mode names are also written out; color is not the only signal. Match phase and score are displayed separately. Use a terminal at least 60 columns × 16 rows; 120 × 35 is more comfortable.

In live-match forms, shared navigation selects fields and the visible **Apply/Create match** and **Cancel** controls. Confirm a text field to start typing; **Enter/Esc** finishes typing, then use navigation keys to select another field. Spaces and navigation letters remain literal text while editing. Select **Apply/Create match** and confirm to save the draft together. **Esc** while typing ends field editing and retains the draft. From field selection, **Esc** or **Cancel** leaves unchanged forms immediately; changed forms ask before discarding, defaulting to **Cancel**. Confirming discard during initial setup returns without creating a match. Team fields open dropdowns; other choice fields use **←/→** or confirmation to cycle choices, including which team scored. The wall-clock timestamp is explicitly editable in RFC3339 form, e.g. `2026-10-03T14:32:08.123+02:00`. The GC countdown has its own explicit `MM:SS` correction field (negative values supported, blank means unavailable). Text-only corrections preserve both timestamps, including wall-clock submillisecond precision. Event-type choices respect the event's own game, even when correcting Golden Goal after the official match has started.

Scores and half headings are recalculated from the active event history, so correcting/removing/restoring a goal also corrects the score. Events stay in capture order: changing a timestamp does not move an entry between halves. Editing metadata updates the heading, but the output filename stays fixed after creation. Changing either team stops the logger's receiver; enable it again from Actions when ready. The GC itself is not changed. Once GC packets or controller-backed history have been captured, changing the tracked team is blocked to prevent reinterpreting existing statistics; create a new match instead. Unrelated edits to legacy metadata preserve its existing perspective.

### Remove and restore events

Choose **Remove selected event**, move from the default **Cancel** to the removal option, and confirm to remove the selected note, goal or marker. It remains in the history, dimmed and marked **[undone]**, and stays saved in the JSON session with its original text, type and timestamp. Undone entries are excluded from the generated Markdown and scores. Select one while idle and choose **Restore removed event** to restore it—even after reopening the session. Removing an actively edited note ends editing; restoring it brings back the event, not an active editor.

Event removal/restore is separate from **Undo note edit / Redo note edit**, which change text in the active note. Text-edit history is temporary; removed events remain persisted. GC score adjustments are reversible too: their signed deltas are saved, and displayed scores never go below zero. Changing an adjustment's event type removes its score delta; text-only corrections preserve it.

## Files and timing

The right third of the screen previews the generated Markdown, including unapplied form edits. It follows the bottom automatically; open the actions menu and choose **Scroll preview up / down**. Scrolling back to the bottom resumes following. The optional Shift-PgUp/PgDn aliases also work inside forms.

Markdown retains the reference log's structure: match heading, robot assignments, half sections, bullet observations, goal lines and scores. New Markdown shows each event's captured GC countdown for match discussion; JSON keeps the full date, precise wall-clock time and timezone offset for robot replay alongside the countdown. Legacy Markdown retains its wall-clock timestamps. This app does **not** synchronize the laptop and robot clocks or control the replay tool.

Each match has three files:

- `.md`: human-readable generated log.
- `.json`: authoritative session data, including match type, each combined entry's game, both timestamps, unfinished notes and all undone events.
- `.lock`: prevents two logger instances from editing the same match. The empty file remains after quitting; its existence does not mean the session is locked.

New notes, typed text and event markers autosave immediately. Form edits apply together when you select **Apply/Create match** and press **Enter**. Saves use atomic file replacement; reopening regenerates the Markdown from JSON if a save was interrupted. Save errors show a red banner and block quitting until a retry succeeds: choose **Retry saving** from the actions menu. Keep the JSON alongside the Markdown for reopening; edit through the app, since the Markdown is regenerated.

## Checks

```sh
cargo test --offline -j 1 -- --test-threads=1
cargo fmt --check
cargo clippy --offline -j 1 --all-targets -- -D warnings
python3 -I -B native/test_transcribe.py
```

The focused contract suite is kept below 1,000 lines, including fixtures and test-only helpers. Rust tests live in `tests/unit/`, included as unit-test modules so production visibility stays unchanged. Coverage focuses on persistence/locking, capture integrity, team perspective, GC handover/reconnect, save/consent guards, recording cancellation, and transcript timing/preservation—not historical bug matrices or cosmetic layout snapshots. All file writes use temporary directories; GC tests use ephemeral loopback endpoints, never real GC discovery addresses. The old terminal-driven UI scripts have been removed.

The microphone-free native recorder check on macOS:

```sh
swiftc -parse-as-library -O -warnings-as-errors -framework AVFoundation \
  native/postgame-audio.swift \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist \
  -Xlinker native/Info.plist -o /tmp/robocup-postgame-audio-helper
/tmp/robocup-postgame-audio-helper self-test
```

This uses generated PCM only; it does not request microphone permission, record, or make network requests.
