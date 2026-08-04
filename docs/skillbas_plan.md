# Skill BASIC Skill Command Plan

## Goal

Implement a Skill BASIC-based alternative to prompt-only pi skills by loading `.sbas` files from `.pi/skillbas/` and registering each file as a slash command:

```text
.pi/skillbas/aichatbot.sbas -> /skillbas:aichatbot
```

Each command runs the corresponding Skill BASIC program, collects `PROMPT` output, and sends that output to pi as a user message with `pi.sendUserMessage()`.

## File Layout

Create a project-local folder:

```text
.pi/skillbas/
  aichatbot.sbas
  review-note.sbas
  summarize-code.sbas
```

The command name comes from the file stem:

```text
aichatbot.sbas -> /skillbas:aichatbot
```

During the transition, `.skillbas` files may also be loaded as a compatibility alias. Prefer `.sbas` for new files.

## `.sbas` File Format

A `.sbas` file is a Skill BASIC program. `PROMPT` is treated as the generated prompt to send back into pi.

Example:

```basic
5 REM AI chatbot. Use when the user wants to chat through Skill BASIC.
10 INPUT "YOU: "; Q$
20 PROMPT Q$
30 END
```

The first `REM` line can be used as the command description. If no `REM` description exists, use a default description.

## Discovery

On extension load or session startup:

1. Find `.pi/skillbas/*.sbas`.
2. For each file, derive a command name from the filename.
3. Register a slash command with `pi.registerCommand("skillbas:name", ...)`.

If dynamic command registration during `session_start` proves unreliable, perform discovery from the extension factory using the current process working directory and require `/reload` after adding files.

## Command Execution

When a user runs:

```text
/skillbas:aichatbot hello
```

The command handler should:

1. Read `.pi/skillbas/aichatbot.sbas`.
2. Pass command arguments as `stdin`.
3. Run the TypeScript Skill BASIC interpreter.
4. Collect `result.prompts`.
5. Report `stderr` via `ctx.ui.notify(..., "error")`.
6. Warn if no `PROMPT` output was produced.
7. Send the prompt output to pi with `pi.sendUserMessage()`.

Initial stdin rule:

```ts
const stdin = args ? `${args}\n` : "";
```

## Prompt Delivery

Initial implementation should send only the `PROMPT` output:

```ts
const promptText = result.prompts.join("\n\n").trim();
if (ctx.isIdle()) pi.sendUserMessage(promptText);
else pi.sendUserMessage(promptText, { deliverAs: "followUp" });
```

This avoids the loop risk caused by calling `sendUserMessage()` inside an LLM-callable tool.

## Optional Skill-Like Formatting

After the basic flow works, consider wrapping generated content in a skill-style block:

```text
<skill name="skillbas:aichatbot" location=".pi/skillbas/aichatbot.sbas">
References are relative to .pi/skillbas.

...PROMPT output...
</skill>

original args
```

Start without this wrapping to keep behavior simple and avoid unnecessary context duplication.

## Safety Rules

- Do not register `/skillbas:name` as an LLM-callable tool.
- Use `pi.registerCommand()`, not `pi.registerTool()`, for commands that call `pi.sendUserMessage()`.
- Keep `skillbas` as an interpreter tool if useful, but do not put `skillbas:name` commands in the active tool list.
- If the agent is streaming, queue with `{ deliverAs: "followUp" }`.

## Verification

Create:

```text
.pi/skillbas/aichatbot.sbas
```

With:

```basic
5 REM AI chatbot
10 INPUT "YOU: "; Q$
20 PROMPT Q$
30 END
```

Then run:

```text
/reload
/skillbas:aichatbot hello
```

Expected result:

1. `hello` is read into `Q$`.
2. `PROMPT Q$` produces `hello`.
3. pi receives `hello` as a user message.
4. No tool-call loop occurs.
