# The harness's UserPromptSubmit hook on Windows: the cognitive-self-check reminder, the same as
# harness-selfcheck-reminder.sh. It is run by `powershell -NoProfile -ExecutionPolicy Bypass -File`,
# which works whichever shell Claude Code runs its hooks in.
#
# Channel: hookSpecificOutput.additionalContext (agent-only), no systemMessage: it fires on every
# prompt, and a bubble each time would be noise. Exit code 0 always: it never blocks the prompt.

$reminder = @"
[self-check reminder] Before you respond, confirm you are running the three cognitive-self-check protocols (~/.claude/rules/cognitive-self-check.md):
  • Protocol 3 (Inbound) — challenge the task BEFORE executing; push-back is not failure.
  • Protocol 1 (Facts) — cite file:line / source verified THIS session; training-data recall is not evidence.
  • Protocol 2 (Decisions) — hack? sane? alternatives? symptom-or-cause? root-cause tracked?
If you have drifted from these over the session, re-read the rule now.
"@

$out = @{ hookSpecificOutput = @{ hookEventName = 'UserPromptSubmit'; additionalContext = ($reminder -replace "`r`n", "`n") } }
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Write-Output ($out | ConvertTo-Json -Compress -Depth 4)
exit 0
