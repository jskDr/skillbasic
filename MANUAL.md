# Skill BASIC Manual

## Run

```console
cargo run                       # interactive REPL
cargo run -- program.bas        # run a file
cargo run -- --engine pi-rpc program.bas
cargo test --locked             # run tests
```

Numbered lines are stored; unnumbered lines run immediately. A line number alone
deletes that line. `?` is an alias for `PRINT`, and `!` is an alias for `PROMPT`.

When stdin and stdout are terminals, REPL commands and BASIC `INPUT` use a
Unicode-aware line editor. Left/right arrows, Home/End, Backspace/Delete, and
in-memory Up/Down history work with Korean and other multibyte text. `Ctrl-C`
cancels the current line and `Ctrl-D` signals end of input. Redirected or piped
input remains supported; malformed UTF-8 bytes are replaced instead of stopping
the program.

Commands: `RUN [file]`, `LIST`, `NEW`, `LOAD "file"`, `SAVE "file"`, `CLEAR`,
and `HELP`.

## Core syntax

Statements: `PRINT`, `LET`, `INPUT`, `IF...THEN`, `GOTO`, `GOSUB`, `RETURN`,
`FOR...TO...STEP`, `NEXT`, `REM`, `DIM`, `DATA`, `READ`, `RESTORE`, `ON...GOTO`,
`ON...GOSUB`, `PROMPT`, `END`, and `STOP`.

Variables are numeric or strings with a `$` suffix. Arrays use `DIM`. Operators
include `+ - * / ^`, comparisons, and `AND OR NOT`. Separate statements with
`:`.

Functions: `RND INT ABS SGN SQR SIN COS TAN ATN EXP LOG LEN MID$ LEFT$ RIGHT$
CHR$ STR$ VAL ASC`.

## Chatbot

```basic
5 REM AI CHATBOT
10 INPUT "YOU: "; Q$
20 PROMPT Q$
30 GOTO 10
```

`PROMPT` (or `!`) prints the AI response and retains bounded recent conversation history.
History resets at each `RUN`.

## AI configuration

The default engine is Ollama. Select pi RPC with `--engine pi-rpc`.

### Ollama

Defaults: `http://localhost:11434` and model `llama3.2`.

Configuration precedence:

1. `OLLAMA_HOST` and `OLLAMA_MODEL`
2. `.env/settings.json`
3. Built-in defaults

```console
cp .env/settings.example.json .env/settings.json
ollama pull llama3.2
ollama serve
cargo run -- samples/ai.bas
```

The JSON file accepts optional `host`, `model`, and `system` fields and is
ignored by Git. Requests time out after 120 seconds. Cloud models and remote
hosts may process prompt data off-device.

### pi RPC

The `pi` CLI must be installed and available on `PATH`. Skill BASIC starts it in
RPC mode with ephemeral session storage and disables its tools and extensions by
default.

```json
{
  "pi": {
    "model": "provider/model",
    "system": "You are a concise assistant.",
    "tools": false
  }
}
```

All `pi` fields are optional. Without a model or system value, pi uses its own
defaults. Set `tools` to `true` only when the model should be allowed to use
pi's configured tools and extensions with your filesystem and process
permissions. RPC calls time out after 120 seconds. Conversation history resets
on `RUN`, `NEW`, and `CLEAR`. Generated pi text is printed and flushed one
Unicode character at a time instead of waiting for the complete response.

## Limits

Skill BASIC is not a sandbox. `LOAD` and `SAVE` use the current user's
filesystem permissions. Version 0.1 does not implement `ELSE`, `%`, `DEF FN`,
graphics, sound, `GET`, or `POKE`. Variables reset at each `RUN`; Boolean
operators are not bitwise.
