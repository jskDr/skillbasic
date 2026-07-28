# Security Policy

## Supported versions

Security fixes are currently applied to the latest revision on the default
branch.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting feature instead of opening
a public issue. If that feature is unavailable, contact the maintainer through
the contact information on the
[maintainer's GitHub profile](https://github.com/jskDr). Include reproduction
steps, affected versions, and the expected impact. Do not include real
credentials or private prompt data.

## Security model

Skill BASIC is a local interpreter and does not sandbox programs. `LOAD` and
`SAVE` use the invoking user's filesystem permissions. `PROMPT` sends the
current prompt and bounded recent conversation history to the configured AI
backend. Version 0.1 uses Ollama; cloud models and remote hosts may process that
data off-device. Future backends must document their own data-handling behavior.
