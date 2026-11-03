# kolum

A kanban client for ticket trackers, in the terminal. Vim keys, everything
rebindable, a Telescope-style fuzzy picker, and issues laid out like the web
app. Jira first; the core knows no provider, so others plug in beside it.

> Early days: the board, the picker, the issue detail and the key engine work
> on built-in demo data and, read-only, on Jira Cloud (see [Status](#status)).

## Try it

```sh
cargo run --release -- --demo
```

`--demo` opens a board of 40 realistic issues from an in-memory provider,
with no account. Press `?` for every key.

## Keys

The defaults follow vim. Counts work on motions (`3j`, `5G`), sequences wait
`timeoutlen` for their next key, and a popup lists what can follow a prefix.

| Keys | Does |
|---|---|
| `h` `j` `k` `l` | previous or next column, card |
| `gg` `G` · `0` `$` | first or last card · first or last column |
| `Ctrl-d` `Ctrl-u` | half a page |
| `Enter` `gd` | open the issue |
| `/` then `n` `N` | filter the board as you type, step through matches |
| `Space f` | find an issue: fuzzy, with `@me` `@name` `#bug` `s:review` `l:backend` `e:PROJ-10` `sp:14` |
| `Space p` | command palette: every action and its keys |
| `Space t` | themes, previewed as you move |
| `Space '` | reopen the last picker as you left it |
| `yy` `yu` `o` | copy the key, copy the URL, open in the browser |
| `ze` | hide or show empty columns |
| `:` | `:theme <name>`, `:help`, `:refresh`, `:q` |
| `q` `Esc` | close the view; `q` on the board quits |

In the picker you type in insert mode; `Esc` goes to normal mode, where `j`
`k` move and `q` closes. `Ctrl-n` `Ctrl-p` move in either mode, `Tab` marks
several issues for one action, `Ctrl-d` `Ctrl-u` scroll the preview, `Up`
recalls past searches.

## Configuration

`~/.config/kolum/config.toml` (or `$XDG_CONFIG_HOME/kolum`). Nothing is
required for `--demo`. Every problem in the file (an unknown setting, an
unknown action, a key that does not parse, one key spelt twice) is listed in a
box at start, and the rest of the file still applies.

```toml
default_profile = "work"

[profiles.work]
provider = "jira"
base_url = "https://acme.atlassian.net"
email = "me@acme.com"               # Cloud: e-mail and an API token
token_cmd = "pass show jira/acme"   # or token = "...", or $KOLUM_TOKEN
board = 42                          # else the first board listed
# flavor = "datacenter"             # guessed: *.atlassian.net is Cloud
# [profiles.work.fields]            # found from /field; force them if wrong
# estimate = "customfield_10016"
# sprint = "customfield_10020"
# epic_link = "customfield_10014"

[ui]
theme = "kanagawa"                # dark light kanagawa catppuccin tokyonight gruvbox
hide_empty_columns = true
card_fields = ["type", "key", "priority", "labels", "epic", "estimate", "assignee"]
detail_fields = ["status", "assignee", "reporter", "priority", "sprint", "epic", "labels", "customfield_10016"]

[theme.overrides]                 # any token, flat or nested
accent = "#7e9cd8"
"status.done" = "green"
label_palette = ["#7e9cd8", "#98bb6c", "#957fb8"]

[keys]
leader = "<Space>"
timeoutlen_ms = 600

[keys.board]                      # global board detail help picker picker_insert command
"J" = "move_card_right"
"L" = "none"                      # unbinds a default

[keys.picker]                     # bindings with a modifier also work while typing
"<C-t>" = "transition"

[picker]
layout = "horizontal"             # horizontal vertical no_preview
```

A theme file in `~/.config/kolum/themes/<name>.toml` names a `base` theme and
the tokens it changes, the same way `[theme.overrides]` does. Theme tokens:
`bg fg muted accent border border_focus selection match_highlight`,
`status.{todo,doing,done}`, `priority.{highest,high,medium,low,lowest}`,
`type.{bug,story,task,epic,subtask}`, `error warn ok`, `label_palette`.

Logs go to `~/.cache/kolum/kolum.log`, never to the screen; `--debug` raises
the level and prints where the file is.

## Architecture

A Cargo workspace whose crate boundaries are the architecture's:

| Crate | Holds | May not depend on |
|---|---|---|
| `kolum-domain` | entities, `RichText`, `Query`, the ports, typed errors | any I/O: no HTTP, terminal or runtime |
| `kolum-app` | use cases: load a board, parse a search, the `Service` the TUI talks through | a provider, the TUI |
| `kolum-provider-fake` | an in-memory provider with demo data | the app, the TUI |
| `kolum-provider-jira` | Jira's REST and Agile APIs; ADF and JQL stop here | the app, the TUI |
| `kolum` | the TUI: keymap, theme, config, picker, views | a provider, except to build one in `main` |

The TUI sends a `Command` and later receives an `Event`; it never waits on the
network. A provider is a crate implementing the ports, plus one line in the
`PROVIDERS` table in `crates/kolum/src/main.rs`.

## Glossary

- **Provider**: one tracker instance behind the ports (the fake one, Jira
  Cloud, later GitHub Projects or Linear).
- **Port**: a trait the domain defines and a provider implements:
  `BoardSource`, `IssueReader`, `IssueSearch`, `IssueWriter`,
  `UserDirectory`.
- **Capabilities**: what a provider can do (sprints, epics, native queries,
  writes…). The UI hides the rest instead of failing.
- **Board**: columns of issues. A **column** holds one or more **statuses**; an
  issue whose status no column holds is not on the board.
- **Issue key**: what a person reads and types (`PROJ-123`). **Issue id**: the
  provider's internal one.
- **RichText**: formatted text as a neutral tree. Every provider's format is
  converted into it; the UI renders only it.
- **Query**: a search in neutral terms (a `Filter`), or `Native` text sent as
  written (JQL on Jira).
- **Context**: where a key binding applies (`board`, `detail`, `picker`…). A
  binding in a specific context wins over `global`.
- **Action**: a named thing a key does (`move_card_right`). The name is all the
  config knows.
- **Picker**: the fuzzy chooser. A **source** supplies its items, preview and
  actions.

## Jira

Jira Cloud is read through REST v3 and Agile 1.0: boards and their column
configuration, issues with their custom fields, comments, history and the
transitions open to an issue. Descriptions and comments arrive as ADF and are
converted to the neutral RichText; what ADF has and RichText does not (an
expand, a media node, a table cell's blocks) is flattened to its words. The
estimate, sprint and epic-link fields are found from `/field` by schema and
name, and can be forced in the profile. Requests are capped at four in flight,
and a 429 is retried after its `Retry-After`.

Data Center is designed in (`flavor`: REST v2, a Bearer token, wiki markup)
but untested against a live server; its wiki markup is shown as plain text for
now. Writes are not built yet: the adapter answers `Unsupported` and says so
in its capabilities.

## Status

Done: the domain and its ports, the fake provider, the key engine, themes, the
validated config, the board, the issue picker, the issue detail, generated
help, and the Jira adapter for reading.

Next, in order: the cache with background sync and remote search in the
picker (`Ctrl-Space`); then transitions (`H` `L` `t`), assigning, comments and
`$EDITOR` edits with optimistic updates; then swimlanes, visual mode,
creating and cloning. Actions bound but not built yet say so when pressed.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
