# Callbacks

An outside program (a CI job, a deploy script, a bot, a scraper) tells an agent that something happened by POSTing JSON to the agent's **callback URL**. The multitool daemon takes every callback. Callbacks of a standard type it delivers itself. A callback of a project's own type goes to a **custom callback**: a handler the project wrote, which stands between the daemon and the conversation (CON-183).

```
outside app ──POST──▶ daemon ──standard type──────────────────────────▶ conversation
                        │
                        └─project type──▶ handler (Rust program) ──POST result──▶ daemon ──▶ conversation
```

Specs: CON-164 (a URL per conversation), CON-165 (`callback_check`), CON-183 (custom callbacks), IF-13, IF-16. Decisions: [ADR 0015](decisions/0015-callback-handlers-are-plugins.md), [ADR 0031](decisions/0031-custom-callbacks-are-project-handlers.md).

## The callback URL

Every conversation has a URL of its own, given by the daemon:

```
http://127.0.0.1:<port>/callback/<token>
```

- It belongs to the whole conversation: it is the same in any tab or window, and after reloads and daemon restarts.
- The token is the only key. Give the URL only to the app that needs it.
- An agent gets its URL with the `callback_url` tool. A user finds it in Settings > Callbacks, where `c` puts it into the composer.

### What an app sends

A POST with a JSON body:

| field | |
|---|---|
| `uuid` | unique per event; the same uuid again is answered `duplicate` and not delivered |
| `type` | `notification`, `journal`, or a type of the project's own |
| `text`, `label` | for `notification`: what the agent reads, and a short name for the source |
| anything else | passed on whole to a custom callback's handler |

```sh
curl -s -X POST "$URL" -H 'Content-Type: application/json' \
  -d '{"uuid": "build-42", "type": "notification", "label": "ci", "text": "build 42 failed"}'
```

### Standard types

These are native: the daemon handles them itself, with no handler.

- `notification` reaches the agent as `[callback:<label>]: <text>`, and starts a turn if the agent is idle. While the conversation is not open, it waits: 500 at most, for 7 days.
- `journal` writes a journal entry and wakes no one.

The replies:

| reply | when |
|---|---|
| `{"result": "handled"}` | delivered |
| `{"result": "queued"}` | the conversation is not open |
| `{"result": "duplicate"}` | this uuid came already |
| 400 `{"error": …}` | the body is bad |

## Custom callbacks

A custom callback serves a type of the project's own, such as `deploy_done`. It lives in the project:

```
.harness/callbacks/<name>/
  config.toml        what it serves and how it runs
  Cargo.toml         a Rust project on the harness-callback crate
  src/main.rs        the handler's code
  target/release/<name>   the built handler, which the daemon runs
```

### A handler is a Rust program

A handler is a compiled Rust program, built on the harness's contract crate, `harness-callback`. The harness does not run scripts as handlers: a handler file that starts with `#!` is refused, with the message 'a handler is a compiled Rust program (CON-183), not a script'. This keeps one way of running a handler, with no interpreters or environments to manage for each language.

The handler's own logic may call anything: a Python script, a shell command, an HTTP API. It can do that through `std::process::Command` or a crate. What the daemon runs, and what answers the daemon, is the Rust program.

### Making one

In the project's folder:

```sh
multitool callback new deploy-done --types deploy_done,deploy_failed
# write the logic in .harness/callbacks/deploy-done/src/main.rs
multitool callback build deploy-done
multitool callback list
```

- `new` makes the folder from the template. Its Cargo.toml takes `harness-callback` by path from `<data dir>/sdk/harness-callback`, which multitool writes from its own build. So a handler is always built on the contract of the multitool that runs it.
- `build` runs `cargo build --release` in the folder. It needs a Rust toolchain. The crate's dependencies (`serde_json`, `ureq`) come from crates.io the first time.
- `list` shows each callback with its types and what keeps it from running (`ready` when nothing does).

### Turning it on: approve and subscribe

The user turns a callback on in **Settings > Callbacks**, under "This conversation's callbacks". One `Enter` on its row (or `Space`, or a click) does it (journal#1382):

- **Off → on.** It approves the handler as it is now, if it is not approved yet or has changed since, and it subscribes this conversation. The approval pins the handler's file by its SHA-256 and means the program runs as you, with your rights. The toast says what was approved.
- **On → off.** It unsubscribes this conversation. The approval stays.
- **Changed.** If the handler's file changes later, even by a new build, the row says 'changed: Enter turns it on again'. The handler does not run until it is turned on again, which approves the new file.
- **A handler that cannot run** (a script, not built, a bad config.toml) is not turned on, and the toast says why.

Subscriptions are per conversation: each conversation has its own pool of custom callbacks. A conversation that is not subscribed does not get the type at all.

When a window opens a project whose handlers wait for approval, a toast says so. An agent sees the same state with the `callback_handlers` tool. Approving and subscribing stay the user's.

### What happens to a callback

When a callback of type `T` is POSTed to conversation `S`'s URL:

1. The daemon looks up `(S, T)` in its registry, a hash table, so the lookup is O(1). The registry is kept in `daemon/callback_subscriptions.json`.
2. If no custom callback is registered for that pair, `T` goes on as before: a standard type, then a condition of `.harness/conditions.toml`. A type that a callback of the project serves, but that `S` is not subscribed to, gets 400 'not subscribed'. Any other type gets 400 'no handler'.
3. If the handler is not approved, or has changed since its approval: 409, and nothing runs.
4. Otherwise the daemon starts the handler and answers `{"result": "started"}` (202) at once. The caller never waits for the handler.
5. The handler runs in a systemd scope of its own (`multitool-callback-<name>-…`) when the user manager is reachable. It is killed when its time (`timeout_s`) is over.
6. The handler POSTs its results to its reply URL. The daemon delivers each one to `S`, and to `S` alone.

Every step is written to the daemon's trigger log, `daemon/triggers.jsonl`: `started`, `ran` or `timed_out` (with what the handler printed, up to 2000 characters), and `handled`, `queued` or `failed` for each result. `callback_check` reads that log.

## The contract

### config.toml

```toml
description = "tells the agent a deploy ended"
types = ["deploy_done", "deploy_failed"]   # the folder's name when none
args = ["--env", "prod"]                    # given to the handler
timeout_s = 30                              # 30 by default, at most 3600
handler = "target/release/deploy-done"      # from this folder; this is the default
```

### The handler's input

| what | where |
|---|---|
| the callback's JSON, as it was POSTed (`uuid`, `type`, all its fields) | stdin |
| `args` from config.toml | its arguments |
| the callback's name (its folder) | `MULTITOOL_CALLBACK_NAME` |
| the callback's type | `MULTITOOL_CALLBACK_TYPE` |
| the conversation it runs for | `MULTITOOL_CALLBACK_SESSIONS` |
| the URL its results go to | `MULTITOOL_CALLBACK_REPLY` |
| the project's folder | `MULTITOOL_PROJECT` |
| working directory | the callback's folder |

What the handler prints to stdout and stderr goes only to the trigger log, up to 64 KB. Results are never read from its output. They go to the reply URL.

### The handler's results

It POSTs JSON to `MULTITOOL_CALLBACK_REPLY`, as many times as it likes until `timeout_s` + 10 s have passed:

| body | what the daemon does |
|---|---|
| `{"deliver": "message", "text": "…", "label": "…"}` | the conversation reads `[callback:<label>]: <text>` (the label is the callback's name by default) and starts a turn if idle; queued while the conversation is not open |
| `{"deliver": "journal", "text": "…"}` | writes a journal entry, tagged `callback` and the callback's name |
| `{"deliver": "none"}` | only notes in the log that the run was handled |

`deliver` is `message` when it is missing. A body may name `sessions`, but only the conversation the run is for: any other is refused.

The daemon answers:

| reply | when |
|---|---|
| 200 `{"result": "delivered"}` or `{"result": "queued"}` | the result was taken |
| 400 | not JSON, no `text`, or an unknown `deliver` |
| 403 | a result for another conversation |
| 404 | no such run |
| 410 | the run's time is over |

### The crate: harness-callback

```rust
fn main() -> std::process::ExitCode {
    harness_callback::run(|call| {
        // call.kind: the type; call.input: the callback's JSON; call.session: the conversation;
        // call.args: config.toml's args; call.project: the project's folder.
        let version = call.input["version"].as_str().unwrap_or("?");
        if call.kind == "deploy_failed" {
            // Its logic may run anything else, in any language.
            let log = std::process::Command::new("python3").args(["tools/last_deploy_log.py", version]).output()?;
            call.message(&format!("deploy {version} failed:\n{}", String::from_utf8_lossy(&log.stdout)))?;
        } else {
            call.journal(&format!("deploy {version} done"))?;
        }
        Ok(())
    })
}
```

| item | |
|---|---|
| `run(handle)` | a whole `main`: reads the call, runs `handle`, exits 0; on an error, exits 1 with it on stderr (the trigger log) |
| `Call::read()` | the call itself, for a `main` of your own; `Error::NotRun` when a variable of the contract is missing, so not run by the daemon |
| `call.name`, `call.kind`, `call.session`, `call.project`, `call.args`, `call.input` | the input above |
| `call.uuid()` | the callback's uuid |
| `call.message(text)`, `call.message_labeled(label, text)` | a message to the conversation |
| `call.journal(text)` | a journal entry |
| `call.none()` | nothing to deliver |
| `call.send(&json)` | any body of the contract |
| `call.reply_url()` | the reply URL |
| `Reply { result }` | `delivered` or `queued` |
| `Error::Refused(status, error)` | the daemon refused the result, with its status and its error |
| `Error::Transport(e)` | the result did not reach the daemon |

## Checking it

- `callback_check` (an agent's tool) sends a test notification to the agent's own URL, and shows what the URL has seen.
- `callback_handlers` (an agent's tool) lists the project's callbacks: their standing, whether this conversation is subscribed, their problems, and the URL.
- `multitool callback list` does the same from a terminal, for the folder.
- The daemon's trigger log, `<data dir>/daemon/triggers.jsonl`, has every step.

## Security

- A handler runs as the user, so it is approved by the user first. It is pinned by its hash, so a changed file needs a new approval.
- Only a subscribed conversation runs it. A handler's results reach only the conversation it was run for.
- It runs in its own scope, with a timeout, and its output is capped.
- The callback URL is local (127.0.0.1) and secret. Do not put it in a repository.
