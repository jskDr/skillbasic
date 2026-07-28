# Skill BASIC Manual

## Run

```console
cargo run                       # interactive REPL
cargo run -- program.bas        # run a file
cargo test --locked             # run tests
```

Numbered lines are stored; unnumbered lines run immediately. A line number alone
deletes that line. `?` is an alias for `PRINT`.

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

`PROMPT` prints the AI response and retains bounded recent conversation history.
History resets at each `RUN`.

## Ollama configuration in v0.1

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

## Limits

Skill BASIC is not a sandbox. `LOAD` and `SAVE` use the current user's
filesystem permissions. Version 0.1 does not implement `ELSE`, `%`, `DEF FN`,
graphics, sound, `GET`, or `POKE`. Variables reset at each `RUN`; Boolean
operators are not bitwise.
