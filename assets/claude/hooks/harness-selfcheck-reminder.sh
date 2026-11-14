#!/usr/bin/env bash
# The harness's UserPromptSubmit hook: the cognitive-self-check reminder.
#
# Fires before the agent answers a user prompt and adds a short, agent-only reminder of the three
# cognitive-self-check protocols, so the agent does not drift from them over a long session. The
# rule itself is ~/.claude/rules/cognitive-self-check.md.
#
# Channel: hookSpecificOutput.additionalContext (agent-only), no systemMessage: it fires on every
# prompt, and a bubble each time would be noise. Exit code 0 always: it never blocks the prompt.

set -u

reminder='[self-check reminder] Before you respond, confirm you are running the three cognitive-self-check protocols (~/.claude/rules/cognitive-self-check.md):
  • Protocol 3 (Inbound) — challenge the task BEFORE executing; push-back is not failure.
  • Protocol 1 (Facts) — cite file:line / source verified THIS session; training-data recall is not evidence.
  • Protocol 2 (Decisions) — hack? sane? alternatives? symptom-or-cause? root-cause tracked?
If you have drifted from these over the session, re-read the rule now.'

if command -v jq >/dev/null 2>&1; then
  jq -n --arg ctx "$reminder" '{ hookSpecificOutput: { hookEventName: "UserPromptSubmit", additionalContext: $ctx } }'
else
  printf '%s\n' "$reminder"
fi

exit 0
