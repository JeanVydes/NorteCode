# Changelog

All notable changes to Norte Code are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A session can be deleted, with everything it left behind. `/delete` in a chat
  removes the session it is on and leaves; `norte-code sessions delete <key>`
  does it from the command line, asking first and taking `--yes` for a script.
  Both go through the service's own delete, so the logical row, the Norte
  session, the node chain and the ledgers keyed by either id go together: the
  tables carry no foreign keys, and a delete that cleared one row alone is what
  left requests pointing at a session the list no longer had.

- A "back to the latest" bar sits on the last row of the conversation
  while the view is scrolled back, naming the key that returns to the tail.
  It is a button too: clicking it jumps to the newest line, so a person who
  scrolled far up does not page back down. `Ctrl-End` does the same from the
  keyboard; the bar is not drawn at the tail, where there is nothing to go
  back to.

### Fixed

- An answer that interleaves with the model's thinking is no longer split. A
  model that reasons emits `Reasoning` and `Text` in turn, and every change of
  block writes the blank row that separates the two; the row was written after
  the entry for the new text had been made, so it landed inside the answer
  instead of between the blocks. The first word of an answer ended up alone on
  its row with a gap under it, and the rest started again below.

- A server switched off in a session takes effect at once. `/mcp disable`
  wrote the config file but the connections were rebuilt from the copy in
  memory, so the switch only appeared after a `/refresh`; the configuration
  is re-read before the rebuild now.

- An MCP server switched off leaves the sidebar. It was drawn as a dead `--`
  row for the life of the process, taking a line of the column next to the
  servers that are actually up; a server that is off is off on purpose, and the
  `/mcp` panel is where every entry, including the off ones, is listed and
  switched. A server that failed to start is still shown, because it is
  configured on and not running.

- Every saved session is in the startup menu, newest first. The list was cut to
  the ten most recent before the menu was built, and the filter only searches
  the rows it was given: typing the key of an eleventh session matched nothing,
  so a session that existed looked absent. The menu draws its own scrolling
  window, so the whole list costs nothing on screen.

- The conversation stays where it is read while an answer streams. The scroll
  offset counts rows back from the end, so every chunk of a growing answer
  shifted the window up and the text under the eye walked away; the frame folds
  the rows it gained into the offset now, so an older line stays put. At the
  tail nothing is folded, because there the newest line is the one to follow.

- North Code refuses to start, with the name of the missing variable, when
  neither `XDG_DATA_HOME` nor `HOME` is set. It used to fall back to a relative
  `.norte-code/sessions.db` that resolved against the working directory: a
  process started without `HOME` — a service, a detached job, a sandboxed agent
  — read an empty database and reported that no session existed.

- Deleting a session removes the Norte session it named, its node tree and its
  cost, telemetry, effect, question and approval rows, in one transaction. The
  delete cleared the key and left everything else behind, so a session gone
  from the list kept its rows in five tables — a database that grew with every
  session forgotten, and the seed of the dangling pointers that made a
  surviving session fail to open with `session not found` or `node not found`.

- A pasted paragraph shows its `[Copied: …]` label again, and the cursor lands
  where it is drawn. The frame was handed the *expanded* paste — what is sent —
  instead of what is on screen, so the label never appeared and the cursor was
  placed by an index into a string that was not the one drawn.

- The input block is as tall as the text it holds. Its height was measured at
  the full terminal width while it is drawn in the body beside the sidebar, so a
  draft that wrapped to two rows in the narrower body was given room for one and
  the last row was cut off. A draft taller than the block now scrolls to keep the
  line the cursor is on in view instead of pinning to its first row.

- The cursor is one box on one row. Every wrapped line whose end the cursor
  clamped to drew its own box; the box now sits only on the line the cursor is
  actually on.

- `ask_user` draws its question. The block was set on the editor's handle and
  nothing painted it — the turn is parked inside the wait, and that wait had no
  draw and no tick of its own — so a question the model asked never appeared and
  could not be answered.

- The modal no longer flickers. Every frame was drawn twice — once for the
  interface and once for the panel — and each draw is its own flush of the
  terminal, so the screen between them was the interface without its panel.
  The two are painted into one frame now, which is what removes the gap; it
  was worst while a panel animated, because every animated frame was two
  writes.

- A modal row shows its text and not the escape codes baked into it.
  `open_editable` did not strip them the way `open_modal` did, so an MCP row
  built with `style.red(...)` showed the SGR code on screen as its
  characters. The strip lives in one place now, where every row is drawn, so
  no caller can leak a code into a panel; the MCP and sidebar rows are built
  plain in the first place.

- The sidebar shows every configured MCP server, not only the connected
  ones. A server turned off, or one that failed to start, was skipped before
  it could become a row, so a person with all their servers disabled — the
  exact case the column is for — saw no MCP panel at all. Each row says
  `on`, `off`, `--` for configured-off or `!!` for a failure.

### Added

- `/menu` returns to the startup screen from inside a session, listing every
  provider and the models each one lists, with the session's own provider marked
  and the current one left in place. `/back` is an alias. It does not drop the
  session: `/restart` is the command that starts a fresh one.

- The input line can select text and replace it. Shift with the arrows, Home,
  End or Up/Down extends a selection from the cursor; `Ctrl-A` takes the whole
  draft. `Backspace` and `Delete` remove the selection whole, typing replaces
  it, and a plain `Left`/`Right` collapses it to its edge instead of moving a
  character. The selection is drawn as the reverse video the mouse selection
  uses. A selection that touches a pasted `[Copied: …]` chip removes the chip
  and not the part of its label it covered, because half the label and half
  what it stands for is the state the chip exists to prevent.

- The telemetry panel assembles itself when it opens: its bars grow to their
  value over a few frames, so the panel reads as something that just appeared
  and not as a screen that was always there. The clock the client already runs
  for the spinner drives it and stops once it has settled. The `providers` tab
  also draws, per provider, a bar of the money reported and, beside it, the share
  of requests the provider priced nothing for — a provider that reports no cost
  is a fact about the provider, not a zero.

- The telemetry panel's `over time` tab draws one sparkline per model — the
  shape of its use across the range's buckets, by the metric in force — with the
  peak named on the line. A day is bucketed by hour and everything longer by day,
  because a day read by day is one point and a year read by hour is thousands
  that say nothing. Models named `provider/model` are never merged.

- `/telemetry` opens a panel over the conversation instead of printing a report
  and losing it. It has tabs: `this session` is the roll-up filtered to the open
  session — its summary, its models ranked, its errors — and `providers` is every
  provider, each expandable into the models it served. A range selector
  (`Today`, `This Week`, `This Month`, `This Year`, `All Time`) sits over both,
  and the ranks are switchable by metric: total tokens, output, cache rate,
  latency, time to first token, cost or errors. Errors are drawn as a bar per
  provider cut into segments by their typed kind, because *which* three errors
  decides what to do about them. `Tab` switches tab, `[`/`]` walk the range, `r`/
  `R` the metric, arrows move the cursor and `Enter` expands a provider. A model
  is keyed by `provider/model` and never merged.

### Changed

- Per-model overrides moved under the provider that owns them. A top-level
  `model_overrides` map keyed `provider/model` was split on its first `/` to be
  read, which is wrong the moment a model name carries a slash of its own —
  `anthropic/claude-sonnet-4` and `deepseek-ai/DeepSeek-V4.1-Flash` are single
  names, not `provider/model` pairs. The block is now
  `providers.<id>.models.<model-id>`, keyed by the model id alone: the provider
  is the block it sits in, so there is nothing to split. A file still carrying
  the old top-level key gets an unknown-key warning from the loader.

- Entries in the conversation are separated by a blank row. A message used to sit
  against the one above it, so a conversation read as one wall of text; now a
  said line, an answer and a notice each start apart from what came before. A
  tool announcement and its output are one call and stay together, and the blank
  row carries no frame, so a selection over it copies a blank line and not part
  of a box.

### Added

- The sidebar shows the connected MCP servers, each with its state and how many
  tools it publishes, under a `mcp on/total` heading. It is read when a session
  opens and after every `/refresh` or switch, so the column and `/mcp` cannot
  disagree about which server is on.

- MCP servers are managed where they are named. `norte-code mcp add <name>
  <command> [args…]`, `mcp enable|disable <name>`, `mcp remove <name>` and `mcp
  list` write the config file, and the same verbs work at the prompt as `/mcp
  add|enable|disable|remove|list`. `enable`/`disable` set `mcpServers.<name>
  .enabled` — the flag the config already carried — so the choice outlives the
  process, and a write at the prompt reloads the connections so the effect is
  visible at once. `add` replaces an entry of the same name, which is how one is
  changed.

### Fixed

- `/refresh` reaches the sessions that are already open. A provider's settings —
  the endpoint, the wire, the headers, whether the prompt cache key is sent — are
  baked into a session's spec when it is resolved, and a reload rebuilt only the
  default one, so a session already open kept using the settings it started with.
  Opting a provider out of `prompt_cache_key` and refreshing left the field being
  sent, and a provider that rejects it refused every turn until the process was
  restarted. A reload now rebuilds every open session's spec from the new
  configuration, keeping each session on the model it was on.

### Changed

- The spinner says `thinking` while reasoning is arriving and `streaming` only
  once answer text starts. Both used to say `streaming` from the first reasoning
  token, which claimed the answer had started when only the thought had.

- `/refresh` rebuilds the MCP connections from the configuration in force, so a
  server added, removed or disabled in the file takes effect without a restart.
  It used to retry only the servers that had already failed at startup, which
  could never pick up a server the file did not name when the process began. A
  server turned off in the panel stays off across a refresh.

- The config names a `default_provider` and a `default_model` instead of one
  `default_model` reference. The old key held two facts in one string and every
  reader split it on the first slash to find them, which is wrong the moment a
  model name carries a slash — NVIDIA names its models `vendor/model`, and so
  do several OpenAI-compatible vendors. Nothing in the code splits a model any
  more: a model is an opaque string, a provider is its own key, and the two are
  written and read apart. An existing `default_model` of `p/m` must become
  `default_provider: p` and `default_model: m`; a config with only one of the
  two is refused with the key it is missing.

- A model reference is a string and never split to find a provider. `/model`
  used to send a name containing a slash as-is and let the service read the
  first segment as the provider, so `nvidia/nemotron` looked like a provider
  called `nvidia` — and a provider whose model names carry the vendor, like
  `nvidia/nvidia/modelo`, was impossible to set. The provider is the session's
  and only `/provider` changes it; `/model` now always prefixes the session's
  provider the way `--provider`/`--model` already did.

- `/provider` switches even when the model the session was on is not in the new
  provider's catalogue. Refusing was the only outcome available, so a session
  could not move to a provider whose catalogue does not list the current model
  even when the person meant to switch model next. It switches and warns, and
  the reference now resolves with conservative capabilities for a model the
  provider lists nowhere. The listing also shows where each provider comes from
  and, when a config entry sits over a catalogue one, which fields it overrides.

- `prompt_cache_key` is omitted unless a provider opts in with
  `providers.<id>.prompt_cache_key: true`. It was sent by default, and an
  OpenAI-compatible endpoint that does not know the field rejects the whole
  request — which reads as the model being unreachable rather than as one extra
  field. Omitting it costs nothing: a provider that honours it falls back to its
  own key. `--disable-prompt-cache-key` is now the default and redundant.

### Added

- A model's answer is drawn with more of its markdown: `~~struck~~` and
  `<u>underlined</u>` now render, and a pipe table is drawn as aligned columns
  with the header in bold and a rule under it, instead of a wall of pipes. The
  width is the widest cell in each column, so a cell that is longer than its
  header is not wrapped. A line with a pipe in it and no separator under it is
  still prose, so a shell pipeline in an answer is not read as a table.

- `/tools` and `/mcp` are panels a person acts on, not text to read and
  dismiss. The tools list is where a tool's rule is decided: Enter walks the row
  through its three states — allow, deny, and "ask again", which forgets the
  rule in memory and on disk so the approval mode decides the next call — and
  `Ctrl-G` flips whether the rule binds this session or every one. A rule set by
  hand used to be one nobody could take back without editing the config; the MCP
  list switches a server off and on where its name is, instead of typing
  `/mcp disable <name>` from memory. The row says what the rule is and the
  footer says what the keys do.

### Added

- A model's answer is drawn as markdown. The rendering was deleted with the
  printer it belonged to — `markdown.rs` and `code.rs` wrote SGR codes into a
  string, which a frame shows as the characters they are — and the answers came
  out with their markers: `**bold**` arrived with its asterisks, a fence arrived
  as three backticks, and a list arrived as a column of dashes. It is back as
  **runs**: `norte-code-cli/src/markdown.rs` renders a whole answer into text
  plus an emphasis, and the view turns the emphasis into a colour, which is the
  rule every other row in the transcript already follows.

  What is drawn: headings without their hashes, lists with a bullet and their
  numbers kept, quotes with a bar, rules, links showing what they read as *and*
  where they go — a terminal cannot click one — and fenced blocks with the
  language on the rule. Inside a fence nothing is markdown, and the code is
  highlighted for the languages it knows: strings, comments, numbers and
  keywords. A language it does not know is drawn plainly, because a wrong
  colour is worse than none.

  What it does not do: tables, footnotes, HTML and nested emphasis inside a code
  span. A terminal has no room for the first three, and the fourth is where a
  hand-written markdown renderer starts to be wrong. A construct it cannot pair
  is printed as it arrived — an unclosed `**` is two asterisks — because
  swallowing it would change what the line says.

### Added

- `/telemetry` reports p50 and p95 beside the average latency. An average is
  dragged by a handful of slow calls and says nothing about the request a person
  is waiting on now: this machine's own ledger reads 6.0s average, 4.0s p50 and
  16.9s p95 over the same window, and only the last two say what a slow request
  costs. The JSON gains `p50_ms`, `p75_ms`, `p50_ttft_ms` and `p75_ttft_ms`
  beside the `p95_*` it already had. One `percentile` serves every level of both
  metrics, where two hand-written copies of the same five lines used to.

### Added

- A session remembers the directory it was working in, and `resume` goes back
  there. The tools are rooted at the process's own directory, so resuming a
  session about one tree from inside another pointed every read, write and
  command at the wrong one: the session looked resumed and answered about files
  it had never seen. The directory is stored with the session and read before
  the service is built, because a service built in the wrong tree has already
  decided what its tools point at. A session stored before the field existed has
  none and opens where the command was typed; a directory that is gone is
  reported and the resume stays where it is.

### Added

- Commands are declared once, in a registry, and the parser, the completion
  and `/help` all read that declaration. They used to be five lists kept in step
  by hand, and they had drifted: `/tools` was parsed and never completed, and
  `/help` named four commands fewer than the client answered to. The crate that
  holds it is `norte-code-commands`, and it knows nothing about what a command
  does — a registry a plugin can add to cannot be keyed by this binary's own
  enum.

  The completion finishes partial arguments, which is what the registry is for:
  `/reasoning xh` completes to `xhigh`, and the hint says which word Tab is
  about to write.

- `ui`, a section of the configuration for what the client shows, holding
  `ui.verbosity` and `ui.diff_style`. `verbosity` is the `--verbose` level the
  client works at when the flag is absent; the flag still wins for one run, and
  leaving both out keeps `medium`, which is what the client always did.

  `diff_style` says how an `edit` call's diff is drawn: `unified`, the new
  default, writes it the way a patch reads — one column, `-` for what left and
  `+` for what arrived, which fits a terminal that is not wide; `split` is the
  side-by-side view that was the only one before. Both name the same edit and
  carry the same line ranges.

- Two tabs join the footer's row: `tools`, which counts what the model can reach
  and how a call to each is decided, and `mcp`, which counts the servers behind
  some of them and says when one failed to start. The tab that was `chat` is now
  `stats`, because it shows what the session has spent rather than the
  conversation — the conversation is the scroll region above, at every tab.
  `chat` still reaches it by name. `Ctrl-1` to `Ctrl-4` go straight to a tab,
  and `/tab <name>` goes to one without a modified key.

  A tab has one row, so each one counts, and `/tools` is where the names are: a
  summary saying three tools are denied is useless without saying which three,
  and the list of them does not fit in the footer. Every tool is printed with
  what happens to a call to it — allow, deny or ask — and how far its rule
  reaches, which is what tells a rule of this session from a rule of every
  session. The counts are read from the engine, the only place that knows
  `deny` beats `allow`, so the row and the engine cannot disagree.

- `norte-code` with no arguments opens the startup flow: a menu of *New
  session*, *Last session* and the saved sessions, and then the editor. The
  session is created before the first message, so the key shown in the footer is
  the one the message goes to rather than one invented as a side effect of
  sending it. `resume` with no key reaches the same menu; `resume <KEY>` keeps
  its old path, `--jsonl` included.

- The startup flow asks which provider and which model when `default_model` is
  not configured, listing the providers named in the configuration first and the
  catalogue's after them — the catalogue is the whole of models.dev, and
  printing all of it buried the two or three providers a host actually has. A
  bounded list says what it left out, and a name past the bound is still
  reachable by typing it. The choice is written to the config file the process
  was started with (`--config FILE`, else the platform user path) as
  `default_model`, so the question is asked once; a file that cannot be written
  is reported and the session opens on the chosen model anyway.

- `norte-code ./some-dir` works in that directory. It is a positional and not a
  flag, because it is the one thing a person names when they name anything, and
  the whole point is that `norte-code ~/src/thing` means what every other
  command line means by a leading path. The process itself moves, before the
  configuration is read, so the project config that applies is the named
  directory's own. A directory that does not exist is reported rather than
  silently ignored, because a typo that leaves the tools reading a tree nobody
  named is the failure that is hardest to notice. The footer names the directory
  in front of the session line, with the home directory abbreviated to `~`.

- `/rename [title]` names the session. With a title it writes what was typed and
  calls nothing: a person who has already decided what the session is called
  should not wait for a model to agree. Bare, it asks the model from the first
  message the session holds, which is the path a session that predates titling
  takes — the history is read out of the engine, because the transcript belongs
  to it and not to the client.

### Fixed

- Selecting text with the mouse copies it. Asking for the wheel turns off the
  terminal's own selection, and a client that takes the mouse and gives nothing
  back is one a person cannot copy out of: the drag is read by the client now,
  drawn in reverse video, and letting go puts it on the terminal's clipboard
  through OSC 52 — which is the clipboard the person is actually using, over ssh
  and inside tmux as well as here. What is copied is the rows the frame painted,
  so a drag that starts on a blank row above the text copies the text.

- The scroll did nothing. The offset counts from the end — the page-up
  direction, towards what is older — and the delta was subtracted the other way
  round, so `0 - 24` saturates back to zero: `PageUp` and `Ctrl-Home` were
  no-ops until something else moved the offset first. A move also writes nothing
  unless the offset changed, and a resize is a change like any other: nothing
  marked it, so the frame was not redrawn and the sidebar kept the columns of
  the size before it.

- The spinner counted `0s` for the rest of an answer. Two places raised the
  phase to `streaming` without setting the instant the wait began, and a tool's
  output clears that instant mid-turn, so the clock the row counts on was `None`
  and the frame read it as zero.

- A tick with nothing to draw wrote a frame anyway. The renderer reads the
  "something changed" flag the keyboard sets — fourteen setters wrote it and
  nobody read it — and asks whether anything is animating. Idle, the client
  writes nothing: measured at 0 bytes per second where it had been writing a
  cursor hide/show pair sixteen times a second.

- The chrome wrote escape sequences into text the frame draws. The status line,
  the usage rows, a notice and a session line are built by the string painters
  and those wrap text in raw SGR codes; a frame draws text, so the codes were
  shown as the characters they are and were counted as characters by the row's
  own cut, which chopped a line that fitted mid-field.

- The status row's rule was drawn onto the status row and then overwritten by
  it, so half of it survived as stray dashes across the bottom line.

- The conversation painted only the rows it had. The area is fixed and the
  content is not, so the rect moved down the screen as the conversation grew and
  left what was above it as the last frame had put it: on a real terminal, the
  tail of an older answer interleaved with the new one.

- The interface drew escape sequences as text. A row of the conversation was
  built with the string painters — `style.red("…")` and the rest — which wrap
  text in raw SGR codes. That is right for a printer writing to a terminal and
  wrong for a frame that draws text: `edit` came out as
  `38;2;224;108;117m- old line`, the shell box drew a `│` down one side of its
  own output, and nothing lined up. A row is now **text plus an ink**, and the
  widget is what turns the ink into colour. Nothing that reaches a frame carries
  an escape sequence.

- A tool's output is one box with rules above and below, and not a box drawn
  with sides. The sides were what a person selected along with the output they
  meant to copy, and they were drawn a row at a time, so the edge landed in the
  middle of the work it was meant to bound. A streamed command also added one
  transcript entry per line; it now collects its rows and adds a single entry
  when the call closes, which is what made `shell` bloat the conversation.

- Wrapping lost the indentation. The wrap split on `' '` and dropped the empty
  runs leading spaces produce, so `    let x = 1;` came back flush against the
  margin: every diff lost the shape it is read by and every nested line of
  output lost its nesting. Indentation is preserved, a wrapped continuation is
  indented under the line it belongs to, and a run of spaces inside a line is no
  longer collapsed.

- Scrolling past the top emptied the screen. The offset was clamped to the whole
  conversation instead of to the last screenful, so a held `PageUp` produced
  `total - total` rows to draw and left no line to scroll back from. It now
  stops with the first row in the first row of the view.

- `resume` left the interface on the terminal and blocked. It never took the
  terminal over the way a new session does, so there was no alternate screen,
  no raw mode and no frame: `draw` returned early, the input box was never
  cleared when a message was sent, and the reader waited on a keystroke the
  terminal was still holding for a line. A resume now enters the guard, exactly
  as `new` does; a resume with no terminal (`--jsonl`, a pipe) keeps the plain
  path and draws nothing.

- A resumed session showed `auto` in the footer while running on the reasoning
  setting it was left on. The setting was stored and the next turn uses it — the
  override a `/reasoning` leaves survives a restart — but the footer was set
  from the configuration's default before the session was named, and nothing
  refreshed it. It reads the session now, which is what the footer has always
  shown after a `/model`, and what `/debug` reports.

- The approval mode went looking for a tool called `command`, and no tool has
  answered to that name since the shell became one tool: the default mode asked
  about a tool that never ran and let the one that did run without asking. The
  shell now asks before it runs a command, which is what `ask`, `auto-edit` and
  `yolo` have claimed all along. `skill_script` was the other name in the same
  list and is a real tool, so it still asks.

  The effect journal carried the same wrong name, so a command a crash may have
  run was never journalled and a replay would have run it a second time.

- The question the model asks is drawn as soon as it arrives, and the box it
  opens answers with what was typed. Both were broken in the list the arrows
  walk: the choices were drawn only after a keypress, because the block that
  holds them was repainted on a keystroke and not on the question arriving, so
  the first thing a person saw was an empty block; and Enter with the box open
  did nothing at all, because it checked the row the cursor was on — still the
  "something else" row — and reopened the box it was already in.

  The list is also what the answer was taken from: any event arriving during the
  wait closed the question and answered with whatever row the cursor happened to
  be on. Nothing but the key that answers now takes the question away, and every
  way out of the wait clears the block.

  The block gives each part its rows instead of drawing until it fills: a long
  question or a long list pushed the box and the hint off the screen, which is
  how the answer came to have nowhere to be typed. A question cut short says so,
  and so does a list with choices past the bottom.

### Changed

- One tool call is one line. A call used to be two rows — one announcing it and
  one reporting how it went — so a suite of ten calls put twenty lines in the
  conversation and the reader had to pair them up by the id beside them. The row
  now says what the call is doing while it runs, with a glyph that turns and the
  time it has taken, and the ending rewrites that same row: the tick, the cross
  or the circle, in green, red or the quiet yellow of a call that was let go of.
  `low` shows neither, which is what "the answer alone" means.

- The context dial warns in steps. It reads its scale from the compaction
  threshold and not from the model's window — the same share of a small window
  and a large one means the same thing — and turns green, yellow, orange and red
  as the window fills towards it. A threshold written into the config and picked
  up by `/refresh` moves the colour with it, which it did not: the two
  disagreed, which is the one thing a warning must not do. The suggestion above
  the input reads in the colour of the rung it names.

- The whole interface draws with `ratatui`, and nothing else in the client
  writes to the terminal. The transcript is data the client keeps — one entry
  per thing that was said or done, wrapped when it is drawn and not when it
  arrives — and the frame is a function of that state, so nothing can land on
  top of anything. The layout divides the screen in constraints from the top
  down, which makes a resize one more frame instead of a migration.

  Commands that report something — `/tools`, `/debug`, `/help`, `/mcp`,
  `/jobs`, `/revoke` — open a panel over the interface instead of printing into
  the conversation. Escape closes it, the arrows walk it, and its bottom border
  says how much of the report there is: a printed report was a report that
  buried the work it was about.

  The reasoning is a boxed gutter in the transcript, twelve lines at most, and
  only at `--verbose high`: it is the longest thing a model writes and the
  least of it is worth reading. Below that the spinner says the model is
  working, which is what the animated bar was for — and the bar is gone, thread
  and all, because a thread that repaints a line of its own is a second writer
  on a terminal that has one.

  The mouse wheel scrolls the conversation, and the keys still do:
  `PageUp`/`PageDown` move a screen, `Ctrl-Home`/`Ctrl-End` go to either end. A
  terminal sends nothing for a wheel event unless the client asks for mouse
  capture, which is why the wheel did nothing at all before.

  Two files of drawing code went with it: `code.rs` and `markdown.rs`
  highlighted syntax for a printer that no longer prints, and `input.rs` lost
  the row arithmetic it used to place the draft.

- The client draws with `ratatui`. The startup menu is the first screen to move:
  it was a hundred lines of hand-written escapes that moved the cursor by a
  count they kept themselves, which is a class of bug this repository has hit
  more than once. It is now a widget — a title, a list and a hint in a layout
  that divides what the terminal has — and the escapes are gone.

  Nothing about how it looks changes yet. This is the ground the rest of the
  interface moves onto: one owner of the terminal, one frame at a time, and the
  layout deciding where things go instead of each writer negotiating it.

  `crossterm` is pinned to 0.28, the version `ratatui` 0.29 depends on. Two of
  them in one binary would be two readers of the same descriptor and two owners
  of the terminal's raw mode.

- The footer and the per-request line say less at `medium`, which is the level a
  person works at. The footer drops the request count, the input and output
  totals and the input:output ratio; the line under each answer drops the same
  counts along with the wall time, the first-token wait and the tools. What
  stays is what is read while working: the context dial, the cache share, the
  cost, the model and the tokens per second. `--verbose high` shows everything
  it showed before.

- A question from the model is a list the arrow keys walk, drawn where the draft
  is drawn. The "something else" row opens the input box, and until it is chosen
  there is no box and nothing is prompted for: the choices are the answer, and a
  second prompt asking for text after the first one already asked for text was
  asking one question twice. Space marks more than one choice when the model
  allows it, and Escape leaves the question unanswered.

- The footer leads with the session's title and keeps its short key in brackets,
  because the title is what a person recognises a session by while the key is
  what `resume` is given. A session with no title yet leads with the key alone,
  which is what it had before one was asked for.

- The reasoning effort is a bar rather than the word `think <effort>`: the word
  had to be mapped to an order in the reader's head, and the bar's colour says
  which rung it is on at a glance. `off` is an empty grey bar, the lowest efforts
  fill one cell, and the bar turns green, yellow, orange, red and purple as the
  effort climbs to `max`. `auto` and a token budget draw no bar, because neither
  is a rung.

- A session is named after its first message. The title is asked for by a
  request of its own — a scratch session holding one instruction, no tools, no
  reasoning, a capped output — so the naming never enters the conversation it is
  naming, and the model that answers may be a cheaper one (`titling.model`).
  `titling.enabled = false` turns it off. The reply is sanitised before it is
  stored: a title reaches a footer and a startup row, so control characters and
  bidi overrides are dropped, only the first line is kept, and a reply that is
  not a name leaves the session with its key.

- The startup menu and the model picker are walked with the arrow keys and
  Enter instead of answered with a number. Typing narrows the list, which is
  what the numbered prompt's typed name became; when the filter matches nothing,
  Enter hands the typed text back, so a provider the catalogue does not list or
  a session key that fell off the list is still reachable. The list scrolls
  around the cursor and says how many entries it left out.

- `run` is `new`. It starts a session and stays in it, which is what the command
  does now: the prompt argument and `--session` are gone, because a session is
  read from the terminal rather than handed a message on the command line.
  `--model` and `--provider` still override what `default_model` would have
  chosen, so `norte-code new --model openai/gpt-5` opens on that model while a
  bare `norte-code` opens on the configured default. Scripts that sent a prompt
  have the protocol client and `--jsonl`, which this command no longer carries.


- A `browser_use` screenshot reaches the model as an image. The tool answers
  with the PNG as base64 inside its result, and a tool result carries text only
  on the `chat` wire, so the model was told how large the capture was and never
  saw it. The host now takes the capture out of the result, bounds it, stores it
  and attaches it as a user message — which carries an image on every wire. A
  model that does not take image input is not sent the image at all: the capture
  is dropped with a warning rather than attached to a turn the model would
  refuse. `tools.screenshots.enabled`, `max_dimension` and `max_bytes` control
  it; `enabled: false` restores the previous behaviour.

- `model_overrides.<provider/model>.modalities` declares what a model accepts
  on input and produces on output. The catalogue is the source for a model it
  lists, but `river` and every other private endpoint named only under
  `providers` has no entry, so its capabilities fell back to text in, text out
  and an image was refused with nothing in the configuration able to say
  otherwise. A list replaces the declared set rather than adding to it, so a
  host can also take a modality away from a model the catalogue overstates; each
  side falls back on its own. The modalities are `text`, `image`, `audio`,
  `video` and `pdf` (`document` is accepted for the last), and an unknown name
  is refused by the parser rather than dropped.

- The session has tabs: `chat` and `todo`, drawn as one row above the
  input. `Ctrl-T` cycles, `Shift-Ctrl-T` goes back, `Ctrl-1` and `Ctrl-2` go
  straight to one, and `/tab [name|number]` does the same for a terminal that
  will not send a modified key. The row says which one is showing and, at its
  end, the key that changes it.
- `/debug` prints what the process knows about itself, built when it is asked
  for: the session, its paths, the catalogue, the model's capabilities, every
  tool it can reach (the MCP servers' included) and the prompt's size. It
  replaces the `--debug` flag, whose output was captured at startup and printed
  where the first answer scrolls it away; a report built at the prompt has the
  tools registered then, the model switched to then, and the servers connected
  then.
- The footer names the reasoning effort the session will ask for, next to the
  session and the token totals. Nothing else on screen says it, so a
  `/reasoning`, a downgrade to fit a new model and a resume were all invisible.
- The tab row can be driven with the keyboard. Pressing Down when the draft has
  no line below it and the history is at its newest entry moves the focus to the
  row; the arrows pick a tab, Enter shows it and returns to the input, and Up
  or Escape leaves without changing anything. Any other key drops the focus and
  is typed, so starting to write from the row does not swallow the first
  character.
- A pasted paragraph becomes `[Copied: Lorem ipsum dolor…]` in the draft, and the
  text is what is sent. What was pasted is longer than the line it is written on,
  and a paragraph dropped into a sentence buries the sentence; the label is how
  the draft stays readable without changing a word of what the model gets. A
  paste short enough to be its own label is left alone. Removing the label — with
  a backspace, a delete, `Ctrl-U` or `Ctrl-K` — takes the whole paste with it,
  because half a paragraph is not a smaller paragraph, and typing inside one
  lands after it rather than splitting it.
- A pasted path to an image is attached to the message and shown as
  `[Image: photo.png]`. A terminal pastes a file's path — kitty does, and so does
  a drag-and-drop — so pasting a picture is how one gets here, and the model is
  handed the image itself: the message carries a `Part::Media` beside its text,
  the bytes are stored in the blob store, and the media adapter resolves them to
  what the provider takes. A path to a file that is not an image is shown as
  `[Path: build.sh]` and attached to nothing — the model can open it with `read`.
  An image pasted while a turn runs is said rather than sent: steering reaches a
  request that is already built, and a request has no place for a part that was
  not in it.
- A `shell` call can run with no time bound: a `timeout_ms` of zero asks for
  none, which is what lets a call start something long-lived — a browser with a
  debug port, a dev server — and leave it running for the rest of the session.
  The zero is granted to a call nobody waits for, which is the one that ends with
  `detach`; a call that holds the turn is given the host's default instead,
  because a turn waiting on a call with no deadline cannot be interrupted. An
  unbounded call is still stopped by `manage_jobs`, `/jobs stop` and `Ctrl-C`,
  and it still dies with the process.

### Changed

- The skills block in the prompt lists names only, not descriptions. A skill's
  description is written for a model to read and they are long — the AWS set
  alone is nineteen thousand characters — and `skill(action: list)` returns the
  same text when it is asked for, so paying for it on every request, every turn
  and every retry, to pick one out of a list a session most often does not need,
  was the largest single item in the prompt that changed nothing. The block says
  how to see what each is for and how to activate one; a session with a big
  installed set cuts its prompt by tens of thousands of characters.
- A detached call is drawn as what it is: `◌ shell let go of in 1200 ms`, not
  `✗ shell failed in 1200 ms`. It stays an error in what the model sees — the
  call did not run to the end — but it did not fail, and a person reading a
  transcript acts on the mark. Which ending it was travels on the wire now,
  instead of the client guessing from the tool's own text, so a timeout, a
  cancellation and a detach stay three different things.
- The shell is one tool with one set of limits: the sandbox, the secrets, the
  output caps and the time bound are the same whether a call is waited for or let
  go of, and `detach` on the request is what says which. There is no second
  declaration to keep in step, and the flag no longer doubles as a per-tool
  limit.

### Fixed

- A browser screenshot is no longer stored twice. The capture used to stay in
  the tool result as base64 *and* reach the model, so every later turn rebuilt
  the history with the full PNG in it: on one measured session that was 3.5 MB
  across twenty results, the largest a single half-megabyte node, read back on
  every request and looked at by nobody. The field is removed from the result
  once the capture is safely attached, and the text the model reads still says
  how large the capture was.

- An attachment the model cannot take is refused before anything is
  committed, so a pasted image no longer poisons the session. The engine
  committed the user's message and only then checked it against the model, so a
  refused part landed in the history first: every later turn rebuilt that
  history, met the same part and failed the same way, which left the session
  unusable — and switching model did not help, because the part is in the
  history, not in the request. The check now runs first, the error names the
  modality and the model, and the session carries on.

- The footer and `/debug` name the same reasoning effort. They disagreed because
  each resolved it separately: the footer used the setting from the command line
  and `/debug` the session's override, which is empty in a session started with
  `--reasoning`, so the footer read `think high` while the report said
  `effective: medium`. The setting is resolved once and both read that value, and
  a `/model` recomputes it because a new model can refuse the effort in force.
- The editor does not panic when a chip is typed in front of. A chip's range is
  an index into the draft, and inserting text moves every character after the
  cursor; the range stayed where it was, so `[Image: photo.png]` with a letter
  before it pointed `[Image: photo.png]`'s range at the letter and the next
  submit walked backwards over the two. Every chip after the cursor shifts with
  the text now.
- A fenced code block is coloured by language and drawn in a box. The fence's
  token — `rust`, `sh`, `python` — selects the grammar; an unknown token is left
  plain rather than guessed at, and a language nothing matches is drawn in the
  box's grey. The colours come from a palette of this project's own, so a
  terminal without truecolour gets a 256-colour or 16-colour code instead of an
  RGB sequence it cannot show.

### Changed

- A code block is dimmer than the answer around it and its box closes under the
  code. It used to open with a `── lang ──` header and never close, which left a
  reader guessing where the block ended, and it was painted in the terminal's own
  foreground, which made it the brightest thing on screen.

### Fixed

- A provider declared only in the config can be switched to. `/model` and
  `/provider` resolved the reference against the catalogue and nothing else, so
  a private endpoint — declared in `config.json` and nowhere else — was answered
  with "unknown provider" while `run --provider` on the same name worked. The
  same reference was resolved two ways. `/refresh` was fixed for this before,
  and three paths were left: the model switch, the summariser
  `config.compaction.model` names, and the reopening of a saved session.
- A session saved on a config-only provider reopens on it. It fell back to the
  configured default model, silently, so work done on a private endpoint came
  back as whatever model was named last and the next message went somewhere the
  work never was.
- A detached call is a job the model can reach, whatever the process does next.
  It was registered only when a chunk of output went past, so a process that said
  nothing after the detach — which is most of them — left no job at all, and the
  model was handed the call's own text with nothing to read the rest from. The
  call's own result registers the job now, so the `Ctrl-G` answer and the
  `detach: true` one end the same way.
- A detached process is reported once. The chunk that closes its streams and the
  call returning are two ends that can see the same process end, and either can
  be first, so the report could be sent twice — two messages about one process —
  or, if the chunk arrived before there was a job to attach it to, not at all.
- The output box of a call that outlives itself stays closed. A detached
  process's last chunk can arrive after the call finished, and the renderer
  opened a second box for the same call, so one call read as two.
- A block of markdown ends where the next one begins. The renderer wrote each
  block as it reached the block's end, and nothing closed the block before it:
  the heading after a paragraph was appended to that paragraph's last line, and
  one paragraph ran into the next. A paragraph had no arm at all, so its text was
  never followed by a break.
- A list item the source puts on the next line stays there. A nested list opened
  with no break before it and put the inner bullet on the outer item's line; a
  blank line between items in the source printed as two.
- A session whose turn ended in an error can switch models again. The engine
  leaves a failed turn `Streaming` so `resume` can pick it up, which is right on
  its own — but the host never lowered it, and a `Streaming` session has no idle
  boundary: every later `/model` and `/provider` was accepted as queued and
  applied by a turn boundary that could not arrive, in that process or the next.
  The host now settles a session whose turn is over — on a permanent failure, on
  a cancel, and on a turn an interceptor stopped — and settles one it finds
  `Streaming` when it reopens it.
- A session a process died in reopens idle. The repair wrote the phase with the
  session's own head, which a commit that carries no nodes rejects, so the write
  failed on every load and the store stayed `Streaming` for the life of the
  session. The repair now leaves the head where it is, which is what a
  phase-only commit means.
- A session can be named by the prefix the footer shows. The footer draws the
  first eight characters of the session id and `resume` took its argument
  straight to the session table, so the name on screen — the only one a person
  has — answered "not known". A prefix that matches one session now resolves to
  it; one that matches several is refused with the ids, because continuing the
  wrong conversation is silent; and one that matches none still says unknown.
- A resumed session gets its task list back. The list is the model's, written
  through `todo` and committed with the conversation, but the state behind the
  footer's widget is in memory: a session opened in a new process drew no tasks,
  and a compaction before the model wrote them again summarised the work without
  the tasks it was tracking. The service reads the last `todo` call back out of
  the history and sends it in the resume's prefix, and hands the same list to
  the compactor.
- A `read` says less at `medium` and nothing at `low`. The page a reader read is
  what the model needs and a person following the work does not: at `medium` a
  long file buried the next call under a screenful nobody asked to see, and at
  `low` — the answer alone — it was printed anyway. `medium` now names the read
  (`src/lib.rs (utf8, 42 line(s), 1200 bytes)`) and leaves the page out; `low`
  shows neither; `high` still shows the whole page.

### Added

- The input line no longer erases the transcript while a tool prints. The
  scroll region ended two rows *inside* the input block, because the reserved
  row count stopped at the suggestion row and skipped the todo row and its gap.
  Every keypress cleared those two rows of streamed output to redraw the draft,
  which made the log flicker and vanish, leaving a hole.
- `Ctrl-G` detaches the running call. The binding was `Ctrl-Shift-B`, and it did
  nothing: it arrived as `Char('b')` carrying SHIFT and CONTROL — the keyboard
  protocol sends the *unshifted* codepoint — while the branch looked for
  `Char('B')`, so the key fell through to plain `Ctrl-B` and moved the cursor
  while the call kept running. Looking for the unshifted form was the first
  attempt at a fix and it did not settle the question either, because whether the
  terminal reports the shift at all, and what it reports, cannot be seen from a
  key that does nothing. `Ctrl-G` is asked for as an ordinary control key:
  crossterm reaches `Char('g')` with CONTROL on both of its paths, the legacy byte
  `\x07` and the keyboard protocol's `CSI 103;5u`. `Ctrl-Shift-B` stays an alias.
- A command's streamed lines are joined before they are shown. A pipe read ends
  wherever it ended, so one row arrives in pieces — `stdout[01] p` and
  `arcial... completa` — and writing each piece as it came put half a word on a
  row of its own. Each stream buffers until it has a whole line. A line that
  never ends, like a progress bar, is shown once it grows past half a kilobyte.
- The output box has a height. Past its rows a command's output is counted
  rather than printed, so a build that prints thousands of lines cannot push the
  transcript off the screen, and the count says how many were left out.
- A command's stdout and stderr are no longer spliced into one row. The two are
  read by separate tasks and arrive chopped at whatever a pipe read returned, so
  a chunk can end mid-word; whichever stream was mid-line is ended first now, and
  each keeps its own rows.
- A command's streamed output is drawn in a box of its own, grey and shaped
  like a terminal session, instead of a `stdout ·` label on every line. It is
  read while it runs:
  ```
  ╭
  │ running 5 tests
  │ test result: ok
  ╰
  ```
  The box is per call and not per stream, because a command's stdout and stderr
  are read by two tasks at once, so a label would name whichever arrived first.
- A command's output is shown as it arrives. The renderer had no branch for
  `ToolOutput`, so a streaming `shell` handed its chunks to the wire and the
  client dropped every one of them: the tool looked stuck until it finished and
  printed its whole result at once. Each line is now written as it is read, and
  the result block at the end no longer repeats text that was already shown.
- A background job's report is written into the transcript when it arrives,
  with the time it arrived. It reaches the model as a steering message the
  human never typed, so nothing printed it before; `SteerApplied` now carries
  the messages beside their ids, which is what lets a client that never saw the
  text show it.
- A message typed while a turn runs waits above the input until the engine
  applies it, instead of being printed as sent. It is not in the history before
  that, so showing it as sent claimed something that had not happened; it is
  written into the transcript — where the conversation reached — when
  `SteerApplied` names it. A report from a finishless message the human did not
  type is shown at the same moment, with its time.
- There is one `shell`, and whether a call holds the turn or is let go of is
  the request's `detach` field. `detach: true` answers at once with a job id —
  the call becomes a job before it has anything to show — and `Ctrl-G` does the
  same to a call that is already running, from the keyboard. Either way the
  process keeps going, `manage_jobs` lists the session's jobs, reads what one has
  written and stops one, and what the process writes from then on reaches the
  model as a message when it ends, so a long build no longer holds the turn. A
  job's output is kept in a bounded buffer while it runs, and the job ends when
  its streams close rather than when its call finishes. `shell_bg` is gone: it
  was the same executor under a second name, and the two had to be kept in step
  everywhere they were named.
- `/jobs [list|check|stop] [job-id]` shows the session's background jobs, reads
  one, or ends one. A job belongs to the session that started it, which is also
  what `Ctrl-C` reaches.
- `/restart` starts a fresh session and leaves the previous one saved and
  resumable.
- `Ctrl-C` stops the session's background jobs while a turn runs, `Escape` asks
  the running turn to stop, and `Ctrl-G` detaches the running call: the turn stops
  waiting, the process keeps running as a job the model can read and stop, and the
  call answers with what arrived so far and the job's name. `Ctrl-G` rather than
  `Ctrl-B`, which readline uses to move the cursor back, and rather than the
  `Ctrl-Shift-B` the binding started as.
- The terminal is put in raw mode with `ISIG` cleared, so `Ctrl-C` reaches the
  editor as a key instead of being turned into `SIGINT`. Before, the signal
  handler and the key reader raced for the same keystroke and the handler did
  not always win, so `Ctrl-C` looked like it sometimes did nothing.
- `--auth disabled`, and `providers.<id>.auth: "disabled"` in the config, sends
  no credential and needs no API key. A local OpenAI-compatible server can be
  used without exporting a dummy key.
- `command`, `write` and `read` show a short result when they finish, so the
  transcript carries what the tool did without `--verbose`: a command's output,
  the path a write landed in with a glimpse of it, and what a read returned.
  The block is bounded and escaped, and a failure now shows its reason at any
  verbosity.
- `/reload` re-reads the configuration without restarting, so a session keeps
  its transcript. Everything the config derives is rebuilt — the default model,
  the keys registered for redaction, the skills and the global approval rules —
  and an unknown key is reported rather than ignored. The open session keeps the
  model it resolved and the rules granted during the run, which are session
  state; a new session uses the new default. A file that does not parse, or
  names a model that cannot be built, changes nothing.
- An MCP server's tools are filtered by `allow` and `deny`, written as the
  server names them. Every tool rides in each request, so the sixteen tools of
  the reference server cost about 25k characters of schema on every turn; the
  lists are what turn "the whole server" into "the tools this host asked for".
  A name the server does not publish is reported at startup instead of being
  ignored.
- `/mcp` lists the connected servers with their tools and whether they are on;
  `/mcp enable|disable <server>` switches one. The process stays up, so turning
  one back on is instant; what changes is whether its tools reach the model.
- MCP servers, named in an `mcpServers` block and each started once at
  startup. Every tool a server publishes is registered as
  `mcp__<server>__<tool>`, so the server keeps its own namespace and a per-tool
  approval rule still names one tool. The block keeps the name and the shape the
  rest of the ecosystem uses, so a configuration written for another client works
  here unchanged. The child gets only its configured `env` and the `PATH`, and
  its values are registered for redaction. A server that cannot start is
  reported and skipped rather than stopping the run, and `--debug` prints the
  servers that connected with the ones that did not.
- `--verbose <LEVEL>` replaces `--quiet` and `--verbose`, which were the two
  ends of one axis and contradicted each other. `low` shows the answer and each
  tool's result; `medium` (the default) also says that a tool is running and
  that the model is thinking; `high` adds the reasoning text, the arguments each
  tool was given and its full output. An `edit` shows its diff at every level.
- `command`, `write` and `read` show a short result when they finish, so the
  transcript carries what the tool did without asking for more verbosity. The
  block is bounded and escaped, a long stdout no longer hides the stderr that
  explains a failure, and a failed tool shows its reason at any level.
- The footer names the session from `medium` up, so returning to one no longer
  means scrolling back to the startup block to copy its key.
- `/reload` re-reads the configuration without restarting, so a session keeps
  its transcript. Everything the config derives is rebuilt and an unknown key is
  reported rather than ignored; the open session keeps the model it resolved and
  the rules granted during the run, which are session state.
- The cache share shows two decimals wherever it is printed: `99.99%`, and
  `100.00%` rather than `100%`. A cache that stops hitting moves by hundredths.
- `--api-key-env VAR` reads the API key from `VAR` instead of the configured
  `api_key_env` or the catalogue's `env` names, without touching the config
  file.
- `--disable-prompt-cache-key` (and `providers.<id>.prompt_cache_key: false`)
  omits the session-derived `prompt_cache_key` field that OpenAI wires send.
  A gateway that rejects unknown fields can now be used.
- Resuming a session shows where the conversation was. The agent replays the
  most recent messages, oldest first, capped at a hundred, so a client that
  starts blank can print the transcript before the turn continues.
- The footer has a context dial: a ring whose filled share is how much of the
  model's window the last request used, blue below the compaction threshold,
  amber near it and red with `compact` once it is reached.

### Fixed

- `/tab` answered "tabs need an interactive terminal" during a turn. That was
  false: the live command handler runs inside the editor, with the footer on
  screen. It switches the tab now, and resolves the argument the same way the
  between-turns path does, so `/tab todo` and `/tab 2` mean one thing.

- The provider was recovered by splitting a `provider/model` label in `/model`,
  `/provider` and the telemetry fallback, which put an empty provider on a row
  whenever a model id carried a slash of its own. They read the spec, which
  holds the two apart, and `/model` with no session no longer lists the
  catalogue under a provider nobody chose.


### Removed

- The runtime-awareness system block is no longer injected at session start, on
  a model switch or after compaction. The model sees the base prompt and
  `AGENTS.md` only. The block was a second system message, which local chat
  templates often reject, and it was not worth its prompt cost.

### Changed

- The workspace gate and the command sandbox live together under
  `tools.confinement`. They answer the same question at two levels: the gate
  re-roots a tool's path under the workspace and refuses an escape, while the
  sandbox wraps `command`'s child process. The sandbox stays off by default and
  paths stay confined by default.
- `tools.confinement.workspace_only: false` lets `read`, `write`, `edit`,
  `glob`, `list_dir`, `grep` and `command`'s `cwd` name any path, for a host
  working across repositories. A special file such as a FIFO is still refused,
  because opening one with no writer would block a tool forever.
- The `--debug` blocks are framed panels whose columns are sized from their
rows, so a long path no longer breaks the frame and the fields of a block line
up whatever the block holds. The section title is bold cyan, a field label dim
and its value in the default colour, and the startup panel carries the session
key, the effective settings and the capabilities in force together. A pipe or a
log file gets the plain rows instead of the box drawing.

### Fixed

- A session's cache share survives a compaction. Three places wrote the
  session's counters from the turn's and none of them carried the two cache
  ones: the write-back after a turn, the reload after a compaction, and the
  summary request's own counters. The telemetry was always right — it records a
  row per request straight from the provider — while the footer, which reads the
  session record, showed the share of a session whose record had been zeroed.
- A resumed session reports the cache share it already had. The session record
  was never given back the two cache counters — the six other fields were — so a
  resuming client computed the share over only the requests made since the
  restart: a session whose prompts had almost all been served from cache showed
  `cache 2.52%` against 5425 requests.
- At `--verbose high`, a compaction shows the summary it produced. It is what
  the fresh session was seeded with and the only record of what the history
  became, so `CompactionFinished` now carries it and the renderer shows it
  bounded and escaped, in a `markdown` block.
- The working bar no longer leaves `waiting for the first token` on screen for
  the whole turn, with a stray copy under it. The bar repaints its own line in
  place, so every writer has to stop it first; two did not — the `thinking…`
  line at `medium` and the request summary — and both wrote onto the bar's line,
  which then repainted a line of its own below. The bar stops before either, so
  it now says `waiting` and then `streaming` on one line and disappears when the
  answer arrives. A shell that streamed its output could glue the same way.
- OpenAI Responses now honors the host's `no_prompt_cache_key` quirk and the
  `--disable-prompt-cache-key` flag. It sent `prompt_cache_key`
  unconditionally, so a gateway that rejects the field could not be used.
- An Anthropic overload or API failure that arrives as a stream event is now
  retried like the same HTTP status instead of ending the turn as an unknown
  error, and a response stopped by the context window shows as truncated.
- Anthropic thinking follows the model instead of using one shape for every
  generation. Claude 4.6 and later take adaptive thinking with
  `output_config.effort`; 4.5 keeps the token budget. An explicit request asks
  for a visible summary, thinking tokens are counted in usage, and cache
  breakpoints never land on a thinking block, which cannot carry one.
- `--reasoning off` is offered on Anthropic 4.5 through 4.8 and Opus 5,
  which accept `thinking: {type: "disabled"}` even though models.dev has no
  toggle for them. Sonnet 5.5 and Opus 5.5 keep refusing it, as the API does.
- Every request now sends the model's catalogue output limit as its max
  output. The adapters fell back to 4096, which capped long answers and made
  an Anthropic reasoning budget of 4096 fail because `max_tokens` must be
  larger than the budget.
- A shell `command` no longer has to name its interpreter (`shell` defaults
  to `sh`), and `write` no longer has to send `overwrite` (it defaults to
  false). The schema required both, so a natural call failed and the model
  spent turns guessing.
- A native Anthropic request that asks for a reasoning token budget now sends
  it as `thinking` instead of dropping it. Models whose catalogue entry only
  offers `budget_tokens`, such as Claude Haiku 4.5, now think when asked.
- Approving a parked tool with `y` no longer closes the interactive session.
  The approval submitted the resumed stream and the turn loop returned before
  driving it, so the session looked parked even though the tool had run.
- A provider that reports cache reads separately, like Anthropic and the
  OpenRouter Messages endpoint, no longer shows a cache share above 100%.
  Input tokens now include the cached and cache-creation tokens, so the
  percentage is a share of the whole prompt.
- A rate-limited free tier is no longer given up on in three seconds. The
  default retry schedule waits 5, 10, 20 and 40 seconds (four retries) under a
  60-second cap, so a per-minute window can reopen before the turn fails.
- A rate limit that says how long to wait is obeyed. Gemini sends the delay
  in the error body instead of a `Retry-After` header, so the CLI used to burn
  its retries and fail while the quota window was still closed.
- Anthropic and Google never report cost, so their live lines no longer say
  it is missing. A USD budget on either warns at startup that it cannot be
  measured and points at tokens or requests instead.
- A queued model switch that could not be honoured no longer breaks the
  session. The model may have left the catalogue, or the session's own
  settings may refuse the switch; either way the turn goes on with the current
  model instead of failing, and every turn after it failed too.
- A model switch and a compaction at the same turn boundary are both reported
  now. The compaction replaced the events of the boundary and the switch was
  lost.
- Text typed while the budget stopped the turn is not thrown away: if it is
  not a new limit, it is steered into the session and the prompt asks again.
- A turn the host ended on purpose is reported as such instead of reading as
  a failure that committed nothing.
- The budget no longer lets a request through that the limit should have
  stopped. The gate measures the request it just let through, in the engine's
  own path; before, the cost was recorded by the service, which runs while the
  engine already asks for the next request, so the budget sometimes answered
  late.
- The numbers a request reports — its tools, its timing — no longer depend on
  whether the service settled the request before or after the answer was
  committed. The telemetry that arrives first waits for the message it belongs
  to.
- The interactive prompt reads again after a turn. When no steered message was
  left to apply, the turn loop restarted with nothing to wait for and never
  returned, so the next line was read by the terminal and never reached the
  service.
- A message typed while a turn runs is echoed into the transcript when it is
  steered. Before, the line was lost from the transcript, so the model's next
  answer read as if it came from nowhere.
- A retry says when the wait is over. The service reports the attempt as it
  starts and the prompt prints `retrying now (attempt N)`, so the last
  `retrying in ...` line no longer stands while the model is already thinking.
- The working indicator no longer paints over the reasoning. It stopped only on
  the first answer token, never on the first reasoning one, so the animated line
  overwrote the dim `┌ thinking` gutter and left the thought cut up. It is now a
  bar that owns its line, is painted through `Style` and is stopped before any
  other writer emits.
- The loading bar no longer carves through the `debug: session` line. It stops
  before the diagnostic is written, like every other writer on stderr.
- `--debug` no longer claims `(200k context, 32k output)` for a model the
  catalogue does not list. It says the capabilities come from the config or the
  fallback and prints the effective limits below, so a `model_overrides` context
  is not contradicted on the line above it.
- A keepalive or the stream's trailing empty part no longer splits a thought
  into several `┌ thinking` blocks. An empty text or reasoning delta changed the
  active block, so the gutter closed and the answer interleaved with its
  reasoning.
- The renderer's own escape sequences are no longer escaped themselves, which
  printed `\u{1b}[1mdebug: model` instead of a styled heading.
- A resumed session announces its turn before replaying the history. The turn's
  task emitted `TurnStarted` only after the replayed messages had been read, so
  the transcript appeared while the client still showed the session as idle.
- A selection of headers are rendered, tables are drawn with borders and
  aligned columns, and emphasis inside a table cell keeps its style. The
  renderer had lost all three: a table's header was dropped, its cells were
  joined at whatever width each happened to be, and a style written inside a
  cell landed outside the row. Inline and block HTML is now shown as the text it
  is and never interpreted, a link shows its destination, an image its source,
  and a `mermaid` block is labelled as a diagram rather than shown as runnable
  code.
- The footer no longer walks down the screen on every resize. The frame that
  moves the scroll region ended with a newline on its bottom row, and each
  newline scrolled the region and left a blank line above the footer; a drag
  fires the event many times, so the gap grew. The frame also released the
  region before setting the new one, which let streamed text land on a footer
  row and be erased by the repaint.
- Markdown answers are escaped before they are rendered. A control character or
  a bidi override inside a model's markdown could reach the terminal raw, and
  one that spelled an escape sequence was turned into a real one by the
  renderer.
- A resumed session shows the conversation, not just the live one. A compaction
  replaces the session with a summary and the last few messages, and the replay
  read only that, so resuming after a compaction printed a handful of lines
  where the user expected the transcript. The chain of sessions a compaction
  came from is walked to fill the window, still without crossing a boundary.
- Resuming reports the usage the session already spent. The counters live in
  the session record and survived, but only the replayed messages crossed the
  wire, so the footer showed the totals of the messages it happened to see: a
  session that had spent a hundred thousand input tokens showed nothing until
  the next request arrived.
- `Shift+Enter` breaks the line instead of submitting. The editor branched on
  the shift modifier, which the terminal never reported: without the keyboard
  protocol it sends the same carriage return for both, so the newline branch
  was unreachable.
- A multi-line message is drawn on its own rows. The input line always drew
  one row and ignored the break, so a second line was invisible and the cursor
  column counted it, which made the draft look like it was erasing itself.

### Added

- A provider can override its wire and credential scheme in the configuration.
  `providers.<id>.wire = "messages"` sends that provider's models through the
  Anthropic adapter and `auth = "bearer"` authenticates a gateway that expects
  `Authorization`; together they let OpenRouter's `/api/v1/messages` endpoint
  exercise the Messages wire without an Anthropic key. `--wire` and `--auth`
  do the same for one run.
- `scripts/install.sh` installs the CLI from the latest release: it picks the
  binary for the system, checks its checksum and puts it in `$PREFIX/bin`.
  The release workflow publishes a binary per platform and the checksums that
  go with them.
- Every request can carry extra headers. `openrouter` gets the attribution
  headers by default (`HTTP-Referer`, `X-OpenRouter-Title`,
  `X-OpenRouter-Categories`), so the app shows up in OpenRouter's rankings,
  and `providers.<id>.headers` in the configuration changes them or adds
  others.
- A session that moved less than one percent of the total is left out of the
  session table, and the report says how many were left out.
- The telemetry report is a table now: a bar shows how many tokens each group
  moved against the busiest one, coloured by the cache — green for the prompt
  the provider served from its cache, red for the part it had to read — and the
  columns carry the requests, the tokens in and out, the reasoning tokens, the
  cache hit rate weighted by tokens, the time to the first token, the latency
  and the cost. The groups are ordered by volume and the time group is named
  after `--bucket`.
- `--bucket week` groups the telemetry by ISO week.
- `Service::with_interceptor` registers an interceptor the engine consults at
  the safe points of a turn. It may add messages to the history or stop the
  turn with a reason, which the client hears as `TurnStopped`: a decision, not
  a failure.
- `xhigh` and `max` are first-class reasoning efforts. The catalogue values map
  to Norte's new variants, `--reasoning` and the config accept them without
  collapsing to `high`, and validation still rejects them for a model that does
  not declare them. OpenAI serializes both, Anthropic sends
  `output_config.effort`, and Gemini uses its `HIGH` ceiling.
- `SetReasoning` stores a session-wide override or a one-message override. Both
  are validated against the selected model before they are saved, survive a
  restart, and are applied by the next turn; a model switch that cannot honour
  the stored setting is refused instead of silently lowering it.
- `Steer` hands a message to the running turn: the engine appends it to the
  history before the next request, so the model sees it there. The message
  stays durable until the engine reports it applied, and whatever the turn
  never reached waits for the next turn boundary.
- `SetModel` is accepted while the session is busy. The switch is validated,
  persisted and applied at the next idle turn boundary, and the request answers
  with `ModelQueued`; `ModelChanged` is emitted when the switch is actually
  applied. An approval or question continuation is not a turn boundary.
- `ListGrants` returns the approval grants a session holds, with their id,
  tool, scope and state, and `GrantList` carries them back.
- The interactive chat accepts `/reasoning`, `/model` and `/revoke` next to the
  commands it already had. Every command with arguments parses into a closed
  set: an unknown setting, scope or argument is rejected instead of being sent
  to the model as a message.
- The input editor suggests the command names and their arguments in dimmed
  text, and Tab accepts the suggestion.
- The interactive footer shows the session's todo list above the session
  totals: the completed count, the current item first, pending items that fit,
  and `+N` for those that do not. It updates when a `todo` call succeeds and is
  cleared by an empty list; model-provided content is folded onto one line and
  escaped before it reaches the terminal. The built-in prompt and the tool
  description now ask the model to call `todo` before multi-step work and after
  each step.

### Changed

- Depend on the Norte revision that explains a literal `\n`, `\r` or `\t` in a
  failed `edit` search and reports an ambiguous exact match as occurrences, so
  the model sees the escaping mistake or the real count instead of only a
  similarity score.
- The built-in system prompt now states the Norte Code identity, requires tool
  use and verification, and tells the model not to guess its harness or claim
  to be another product.
- Terminal styling now uses 24-bit RGB when `COLORTERM` advertises truecolour,
  with the 256-colour palette as the fallback. Themes that remap the classic
  16-colour slots (such as a monochrome kitty theme) previously rendered every
  label in the same foreground colour.
- Tool calls render in blue, and a `command` call shows the command it ran even
  under `--quiet`; a long command is clipped to one line. Cancelling a turn
  before the provider reports usage now explains that the cancelled request is
  still recorded in telemetry.
- The interactive footer shows that a turn is working as soon as it is sent, and
  shows the retry delay and attempt while a transient provider failure waits.
- The local message queue is gone. Text typed while a turn runs is steered into
  that turn: the engine appends it to the history before the next request the
  turn makes, so the model sees it there instead of after the turn ends. The
  message stays durable until the engine reports it applied, and whatever the
  turn never reached waits for the next turn boundary. The commands that make
  sense during a turn are handled straight away. The footer row that showed the
  queue now shows the command suggestion.
- A steering message carries a name. The client may name it, the engine reports
  the ones it applied with `SteerApplied`, and the service drops the durable
  copy of exactly those. The client no longer counts messages to know whether
  one of them landed.
- `budget.action = "ask"` asks for a new limit in the interactive CLI. The turn
  stops uncommitted, the CLI prompts for a limit, and a new limit raises the
  budget and resumes the session. An empty answer leaves the session stopped
  and resumable.
- A second `Ctrl-C` force-quits immediately instead of waiting five seconds, and
  the first one now reports that the running tool is being stopped.

### Security

- Model answers, reasoning, tool output, skill bodies and resources, approval
  previews and catalogue or report text are escaped before they reach the
  terminal. ANSI escapes and bidi overrides can no longer move the cursor,
  rewrite a line, spoof an approval prompt or change the terminal title;
  newlines and tabs are kept.
- Skill discovery refuses a `SKILL.md` that resolves outside its skill
  directory, so a project cannot point it at `/etc/passwd` or another host file.
- `web_fetch` accepts only public HTTP(S) targets. Loopback, private,
  link-local, CGNAT, benchmark, documentation and reserved IPv4/IPv6 addresses
  (including cloud metadata endpoints) are refused, every resolved address is
  checked and pinned for the request, proxies are bypassed, and each redirect is
  validated before the next hop.
- `skill_script` keeps at most the configured output bytes per stream while
  draining the pipes, so a script that floods stdout or stderr cannot exhaust
  host memory before the timeout stops it.
- `skill_resource` reads at most the configured bytes plus one instead of
  loading the whole file, and `SKILL.md` and shebang parsing have the same kind
  of bound, so a large or hostile skill file cannot exhaust memory first.
- Config loading reads at most 1 MiB, API key files at most 64 KiB, and both
  must be regular files. A project overlay can no longer point `api_key_file` at
  a FIFO or an endless device and hang or exhaust the host at startup.
- The catalogue cache and download are read in bounded chunks, so a corrupt
  cache file or an oversized response is refused instead of being loaded whole.
- `grep` reads at most its file cap even if the file grows after the size check,
  so a changing file cannot make the tool allocate without bound.

### Fixed

- An `edit` call always renders each change in its own boxed, side-by-side
  view with original and result line numbers plus two lines of surrounding
  context, including under `--quiet` and when `--verbose` is off. The host
  snapshots the file immediately before the edit runs, so the preview still
  shows the original lines when the tool finishes quickly. A failed edit shows
  its diagnostic in the same modes.
- A forked session receives a fresh engine session id, so it no longer collides
  with its source, and it is recorded as a fork rather than a compaction.
- `glob` and `grep` honor `.gitignore` in a workspace without a `.git` directory
  and skip hidden files and directories.
- Resizing the terminal no longer duplicates or breaks streamed text. Resize
  events are debounced, the footer rows for the old and new sizes are cleared,
  and the scroll region is reset in one atomic write so the renderer never
  continues a line the terminal has just reflowed.
- The catalogue cache writes a unique temporary file with `create_new` and
  renames it, so a concurrent or interrupted write cannot replace a good cache
  with a partial one.
- A global approval update writes the user config atomically, keeps a symlinked
  config a symlink, preserves an existing mode, and creates a new config with
  owner-only permissions.

### Added

- Runtime awareness stored per session: a new session gets a system block with
  the current model, provider and wire, catalogue dates and capabilities,
  modalities, compaction threshold and retained messages, sandbox state,
  approval mode, effective config and a short Norte Code help section. It
  contains no keys or endpoint URLs. A model switch appends a runtime update,
  and compaction seeds the fresh session with a new block.
- The mdBook guide under `guide/`: concepts, task guides and reference pages for
  the service, protocol, sessions, tools, approvals, compaction, budgets,
  storage, skills and CLI. A Pages workflow builds it on pull requests and
  publishes it from `dev`.
- Initial Cargo workspace: the protocol, agent service, config, catalog, cost,
  tools, SQLite store, CLI and TUI crates, with CI, formatting and lint
  configuration.
- The versioned client protocol: requests, server events, deltas, usage and
  budgets, with no Norte types on the wire.
- The in-process service: one Norte engine, logical sessions, streamed turns,
  cancellation, usage settling and reported cost.
- Configuration loading and merging (user file plus project overlay), with
  unknown-key warnings, validation and env/file secrets.
- The provider factory: models.dev `npm` selects the wire, `api` is the
  endpoint and `env` names the key, so any OpenAI-compatible provider takes the
  same path as OpenRouter. Native OpenAI, Anthropic and Google use Norte's
  presets when no URL is configured.
- The headless `norte-code run` command, with streamed text and `--jsonl`
  output, plus `--budget`, `--reasoning` and `--approval-mode`.
- `--interactive` on `run` and `resume`: the same session stays open. A pinned
  footer holds the running session totals, the queued-message preview and a line
  editor. The totals update after every request (requests, input/output tokens,
  average cache share, total input:output ratio and reported cost). Messages you
  send are echoed above the footer; lines completed while a turn is still
  running stay in the queued preview and are sent together as the next message,
  even after a failed or cancelled turn. Arrow keys, Home/End, Delete,
  Backspace, paste and the usual control keys work; a reverse-video cursor marks
  the insertion point and typed text never leaks into the answer. `/exit`,
  `quit`, Ctrl-D or Ctrl-C at the prompt leave; a first prompt is optional on
  `run`. The prompt shares stdin with approvals and questions so typed input
  stays in order.
- `tools.command_max_model_bytes` (default `131072`, range `1024`–`1048576`)
  bounds the bytes of each `command` stream that reach the model; the rest is
  elided around the marker. Previously this was fixed at Norte's 32 KiB default
  and could not be raised from the config.
- The models.dev catalog client: tolerant schema, capability and reasoning
  mapping, ETag/TTL cache with stale and bundled fallbacks, and a live refresh
  request.
- Every provider the catalog describes, not just the native four: `npm` picks
  the wire, `api` supplies the endpoint (including `${VAR}` templates), and
  `env` names the API key; native OpenAI, Anthropic and Google still use Norte's
  presets when no URL is configured.
- `norte-code models`, with provider, tool and reasoning filters plus `--json`,
  and `model_list` over the protocol for the TUI picker.
- Mid-session model switching at a turn boundary, reusing the session and its
  store while rebuilding the engine for the new model.
- Reasoning resolution per model, with effort clamping to Norte's four efforts
  and config overrides that win over catalog data. `--reasoning` accepts
  `auto`, `off`, `minimal`, `low`, `medium` or `high` and is checked against
  the selected model before the turn starts; the implicit default asks for
  `medium` when the model declares it.
- Agent Skills: discovery from user, configured and project roots; YAML
  frontmatter validation against the Agent Skills specification; progressive
  disclosure through the `skill`, `skill_resource` and opt-in `skill_script`
  tools; `norte-code skills list|check`; and `ListSkills`/`RefreshSkills` over the
  protocol.
- A configured tool registry per engine: Norte's `read`, `write`, `edit` and
  `command` (with `PATH` and every resolved API key redacted), plus `glob`,
  `list_dir`, `grep`, `todo`, `ask_user` and opt-in `web_fetch`.
- A workspace-root policy that re-roots tool paths, canonicalizes the existing
  prefix so a symlink cannot escape the root, refuses dangling symlinks, and
  rejects FIFOs, sockets and devices before a tool can block on them.
- `ask_user` parks a running turn on a question and resumes it through the
  `QuestionAsked`/`AnswerQuestion` protocol, with a headless prompt on a terminal.
  The model may pass `options` for a closed selection and `multiple: true` for a
  multi-select; the terminal always appends an “answer something else” entry
  that falls back to the existing free-form answer channel.
- Global approval rules in the user config: `approvals.allow` and
  `approvals.deny` are tool-wide, denial wins over allow, and “always” choices
  written by the CLI preserve the rest of the config file.
- Approval modes (`ask`, `auto-edit`, `yolo`) as a Norte policy, with
  `Approve`/`Deny`/`Revoke` resuming a parked session and `allowed-tools`
  pre-approval behind `skills.honor_allowed_tools`.
- Failed tool calls carry the error text the model saw, shown by
  `norte-code run --verbose`.
- A SQLite `Store` + `BlobStore` that passes Norte's `store_conformance` suite,
  with one transaction per commit, optimistic concurrency and the host's own
  tables for logical sessions, pending questions and settings.
- Durable logical sessions: `run` reopens the database, restores the session
  list, and `--session <key>` continues one; `norte-code sessions` lists them.
- Restart-resume of parked approvals and pending questions. A resumed session
  re-emits the approvals still waiting and a question is replayed by its tool
  call id, so an answer given after a restart reaches the parked tool.
- Approval records and sessions are stored with their grants lifted into a
  side table shape, because `GrantScope`'s internally tagged newtype variants
  are not representable in JSON.
- An effect journal for `write`, `edit`, `command` and `skill_script`. The
  intent is written from the engine's tool-metadata callback immediately before
  the executor runs and the finish is written from the tool event; a call that
  was still at intent when the process started is refused on replay instead of
  being run a second time. Norte guarantees commit-at-most-once, not
  effect-at-most-once.
- Interactive approvals on stdin: a parked call prints the tool and a semantic
  preview of its arguments, then accepts `y` (allow once), `s` (allow this tool
  for the session), `a` (allow this tool always), `n` (deny once), `ns` (deny
  this tool for the session) and `ng` (deny this tool always). A message after
  any denial (`n <message>`, `ns <message>`, `ng <message>`) reaches the model
  as human feedback. Session denials are durable with the session and global
  rules live in the user config. The resumed turn is folded back into the same
  render loop, so a turn can park and resume several times. Piped stdin still
  leaves the session parked rather than guessing.
- Interactive chat commands: `/help`, `/compact`, `/refresh`, `/allow <tool>
  [session|global]` and `/deny <tool> [session|global]`. Rules default to the
  session and are stored with it; `global` writes the user config and replaces
  the opposite rule. An allow rule revokes a previous deny without another
  pending call. `/refresh` re-scans the skill roots so the next turn registers
  the new tools.
- A styled text renderer: model and tool names, outcomes, usage and errors use
  colour when the destination is a terminal, reasoning streams in its own dim
  gutter instead of mixing with the answer, and the run ends with a compact
  request/token/cost summary that includes the cache-read share and the
  input:output ratio. `NO_COLOR`, `TERM=dumb`, `CLICOLOR=0`, pipes and `--jsonl`
  all keep the output plain. With `--quiet`, reasoning is redacted to a single
  `thinking…` marker rather than disappearing entirely.
- Interrupted sessions are repaired on startup: Norte's `streaming` phase
  describes a live process, so a session left there by a hard kill is committed
  back to `idle` instead of being listed as streaming forever.
- A provider-reported cost ledger in SQLite, one row per model request, with
  per-message, per-session and per-UTC-day rollups. A request whose usage
  payload has no cost is counted as "not reported", never priced; catalogue
  prices are ignored. `norte-code cost` prints the same numbers as the run
  summary, with `--json` and `--session` filters.
- Budgets with `message`, `session` and `global_daily` scopes, `usd`, `tokens`
  and `requests` units, and `warn`, `ask` and `stop` actions. A Norte layer
  checks the ledger before each request, a post-request check reports warnings
  and exhaustion, and a stopped turn stays resumable. USD budgets are refused
  over unreported cost, and an incompatible model switch is blocked.
- `norte-code resume <key>` continues a parked or stopped session, optionally
  replacing its budget first (`--budget`, `--budget-unit`, `--budget-action`).
- `Ctrl-C` cancels the running turn through the protocol, so a hanging tool is
  stopped (including its process group) instead of leaving the terminal stuck;
  a second `Ctrl-C` quits. At an approval or question prompt it leaves the
  decision pending and exits with a resume hint instead of guessing.
- `--verbose` prints each tool's effective arguments and returned output as
  fenced blocks, with a visible truncation marker instead of an unbounded dump.
  The run summary also lists every tool that executed and how many times.
- `--debug` prints startup details: session id, model and wire, approval mode,
  iteration limit, workspace, config and database paths, and extension keys. It
  also prints the selected model's modalities, context and output limits,
  tool kinds, reasoning controls, streaming, cache, features, structured output
  and catalogue metadata, the effective reasoning setting for the session, and
  the character length of the system prompt and tool declarations; tokens are
  left to the provider instead of being estimated.
- A system prompt can be set in the config (`system_prompt`) or per run or
  resume (`--system-prompt`); the flag wins over the config, and the config wins
  over the built-in prompt.
- Model iterations are unlimited by default, with `--max-iterations N` to cap a
  run or resume. The compaction threshold can be overridden for one run or
  resume with `--compaction-threshold FRACTION`; it otherwise comes from
  `compaction.threshold` in the config.
- Vendor request extensions via `--extension KEY=JSON` or the config
  `extensions` map, keyed `<wire-family>:<field>`. OpenRouter upstream routing
  works through `openai.chat:provider.order`.
- Local request telemetry: a `requests` table in SQLite records every provider
  attempt with session and model, prompt/output/cache tokens, tools called,
  latency to first output and total, reasoning, reported cost, status and
  metadata. `norte-code telemetry` rolls the rows up by hour, day, month, year
  or total, with top providers, models and sessions. Nothing leaves the machine.
- Compaction at a safe boundary: when the provider-reported context crosses
  `compaction.threshold`, Norte Code summarises the old conversation and seeds a
  fresh Norte session with the summary, the todo list and the last
  `keep_recent` messages. `compaction.trigger` selects `idle` (the default,
  compact at a turn boundary) or `eager` (compact at the first safe
  model-request boundary inside a turn, after a complete tool result). Tool
  calls are never split from their results, eager never runs while an approval
  is pending and runs at most once per turn, the old session is kept for audit,
  the new session id is shown, and the summary request enters the cost ledger
  and budgets. `/compact` in the interactive prompt triggers the same path on
  demand; a failed summary leaves history unchanged.
- Project instructions: an `AGENTS.md` in the workspace root is appended to the
  system prompt of every new session. Only a regular file is read, the read is
  capped at 64 KiB, and the text is treated as repository guidance that does
  not override the user's request, the approvals or the budgets. `--debug` shows
  the file and its size.
- Bounded retries for transient provider failures (rate limits, overload,
  server and transport errors). The wait honours the provider's `retry_after`,
  capped by `retry.max_delay_ms`, and falls back to `retry.backoff_ms` with
  exponential growth when the provider gives no hint. A retry resumes the same
  session instead of resending the user's message, permanent failures (quota,
  authentication, malformed requests) are not retried, and
  `retry.max_attempts: 0` disables retries. Clients see a `RetryScheduled`
  event with the attempt, the delay and the reason.
- Optional command sandbox (`tools.sandbox`): `command` runs through bubblewrap
  on Linux or `sandbox-exec` on macOS with the workspace writable, the rest of
  the filesystem read-only, a private `/tmp` and no network unless enabled.
  `--sandbox`, `--no-sandbox` and `--sandbox-network on|off` override the config
  for one run or resume; the network option needs the sandbox and conflicts with
  `--no-sandbox`. Approvals and the effect journal keep the original command,
  the panel adds a `[bwrap]`/`[seatbelt]` badge, and a missing platform binary
  fails closed instead of running unsandboxed. Request telemetry rows record
  whether the sandbox was on and whether it allowed network, and `--debug` shows
  the effective sandbox state.
- A lost store race is reported as a conflict with a resume hint instead of a
  provider error. The session version is an optimistic-concurrency token, not a
  message count; a turn that loses the race did not commit.
- The per-request footer now includes answer tokens per second, total duration,
  first-token latency and the tools that request called, alongside the existing
  input/output, cache-read share, input:output ratio and reported cost.
  `--quiet` still hides all usage lines.
- The per-request token rate is answer tokens from the first delta to the end
  of the request. The wait for the first token is reported separately as
  latency, and a single final burst falls back to the whole duration instead of
  dividing by a sliver.

### Fixed

- An `ask_user` question is drawn with its choices. The state the frame paints
  had the question hard-wired to `None` and the only reader of the pending
  question was test-only, so the widget never learned one was waiting: with
  choices offered, the frame cut the block to a single row and nothing appeared
  at all. The frame now reads the question the editor owns, the block counts it
  as the prompt plus every choice plus the "something else" row, and that row is
  drawn even when the model offered no choices.

- A streamed tool output box is no longer up to 400 rows tall. It was a fixed
  count, about ten screens on a normal terminal: a long `shell` call filled the
  transcript with one box and there was nothing to scroll but the whole
  conversation. The box now keeps at most one screenful minus the border and the
  prompt, and counts what it dropped, so the last screenful is what a person
  reads and the rest is one line saying how many were left out.

- `grep` and `web_fetch` now escape Unicode bidi overrides and line separators
  the way `read` does, so a crafted line cannot visually reorder or hide the
  rest of a result. `grep` also clips its 500-character line width by
  characters instead of bytes, matching `read` for accented text.

[Unreleased]: https://github.com/JeanVydes/NorteCode/commits/dev
