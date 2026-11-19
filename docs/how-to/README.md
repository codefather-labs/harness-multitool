# How to

Each page here is one task, shown live. On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to), every page plays its own scripted session in the real terminal UI, compiled to WebAssembly. Watch it, or switch to Explore and drive it yourself. Nothing leaves your browser: the model is scripted, and every sign-in and outside event is mocked. Each page also says how to do the same in the harness you install (CON-187).

| How to | What it shows |
|---|---|
| [Start for the first time](first-start.md) | the welcome screen, the start page, a new session and its status line |
| [Sign in to each adapter](sign-in.md) | Claude Code and Codex signed in by their own programs, ChatGPT by OpenAI's page, an API key |
| [Configure the harness](configure.md) | the providers and models, profiles, themes |
| [Move a conversation to another adapter](move-conversation.md) | Claude Code to Codex to Ollama and back, the conversation picked up after each move |
| [Plan before building](plan-mode.md) | plan mode with its journal entry, mind map and flowchart, approved in the window and on Telegram |
| [Let agents talk to each other](agent-to-agent.md) | two tabs, two agents: one writes, the window switches to the other, the answer comes back |
| [Orchestrate many agents](orchestration.md) | a master starts three slaves in (S) tabs, checks their proof and closes their tasks |
| [Orchestrate subscription CLIs through one daemon](subscription-clis.md) | Claude Code and Codex sessions on the user's own subscriptions, one daemon, one shared journal, agent to agent |
| [Debug a pipeline step by step](step-debug.md) | step-debug mode and a live flowchart |
| [Callbacks by design: standard and your own](callbacks.md) | the callback URL, a notification from CI, and a project's own handler in Rust turned on with Enter |
| [Work from Telegram](telegram.md) | messages, files and plan approval from the phone |
| [Keep the knowledge graph](knowledge-graph.md) | a work branch from todo to proof and close, and journal search |
| [Rely on the daemon](daemon.md) | connecting, a lost daemon coming back, a conversation that went on meanwhile |
| [Carry a conversation anywhere: dump and load](chats.md) | a conversation dumped with its journal and maps, loaded as a new one, and going on where it was |
| [Work in tabs](tabs.md) | tabs, nicks, switching, and a busy tab behind |
