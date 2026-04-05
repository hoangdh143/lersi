# TODOS

## Core

### SQLite internal_error handling

**What:** Wrap all `db.rs` calls in `Result`, convert `rusqlite::Error` to `{ "error": "internal_error", "message": "<detail>" }` JSON response across all MCP tool handlers.

**Why:** Currently unhandled SQLite errors (disk full, locked DB, corrupted file) panic the MCP server process and crash the Claude session. Users lose their active learning session.

**Context:** All tool handlers currently call `db.rs` functions that return `rusqlite::Result`. Errors propagate to `main.rs` as a panic. Fix is: match on `Err` in each handler and return a structured error JSON instead. Start in `src/db.rs` with a wrapper type, then update each handler. Add `internal_error` to the error codes list in README.

**Effort:** S
**Priority:** P1
**Depends on:** None

---

### Schema migration versioning

**What:** Add a `schema_version` table with an integer version. On startup, read the current version and run pending migrations from a `MIGRATIONS` array.

**Why:** Without schema versioning, any v2 schema change (FSRS fields, junction table for prerequisites, new columns) would silently break existing `.db` files. Users would either get cryptic SQL errors or corrupt data.

**Context:** Add to `src/db.rs`. Pattern: `CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL); INSERT OR IGNORE INTO schema_version VALUES (0);` then check current version and run migrations in order. Keep migrations as `&str` constants. First migration is the current v1 schema. This is needed before shipping any v2 feature that touches the schema. Do before v2, not necessarily before v1 launch.

**Effort:** S
**Priority:** P2
**Depends on:** None

---

## v2 Features

### FSRS spaced repetition algorithm

**What:** Replace SM-2 with the FSRS-5 algorithm for more accurate review scheduling.

**Why:** FSRS is measurably more accurate than SM-2 at predicting forgetting. It requires additional fields: stability, difficulty, elapsed_days, scheduled_days. Better scheduling = better retention = happier users.

**Context:** SM-2 is implemented in `src/sm2.rs`. FSRS is a drop-in replacement with the same inputs/outputs. Requires a schema migration (new columns on `concepts` table). FSRS-rs crate exists on crates.io. Do after SM-2 is validated in production use.

**Effort:** M
**Priority:** P3
**Depends on:** Schema migration versioning

---

### Optional server-side curriculum generation

**What:** Add optional `LERSI_LLM_API_KEY` env var and `LERSI_LLM_BASE_URL` for server-side ConceptGraph generation. When configured, `learn__start_topic` accepts `topic` + `prior_knowledge` without a `concept_graph` param and generates it internally.

**Why:** Zero-config UX — user can say "teach me Rust" without the AI client needing to generate a structured graph first. Lowers barrier to first use.

**Context:** Current design: client-side generation only (AI client generates ConceptGraph JSON). Server-side generation is additive — keep client-side as the primary path, make server-side optional. Use OpenAI-compatible API (covers OpenAI, Anthropic, local Ollama). Generate concept list with structured output mode.

**Effort:** M
**Priority:** P3
**Depends on:** None

---

### Windows cross-compilation

**What:** Add Windows x86_64 binary to GitHub Releases. Build via `cross` with `x86_64-pc-windows-gnu` target + MinGW sysroot Dockerfile.

**Why:** Expands user base to Windows developers.

**Context:** `rusqlite` bundles SQLite via `libsqlite3-sys` which requires a C toolchain. Linux cross-compile to Windows needs `mingw-w64` in the Docker image used by `cross`. See: `cross` Dockerfile documentation for MinGW setup. Defer until macOS and Linux binaries are stable.

**Effort:** M
**Priority:** P3
**Depends on:** None

---

### Anki .apkg export

**What:** `lersi export <topic> --format anki` generates an Anki deck package (`.apkg`).

**Why:** Users who also study with Anki can sync their Lersi concepts into their existing flashcard workflow.

**Context:** Anki `.apkg` is a SQLite database with a specific schema. Rust crate `anki-db` or custom implementation. Each `Concept` becomes a basic front/back card (title + summary). Mastery scores and SM-2 data are not Anki-compatible — export is one-way. JSON/Markdown export is already in v1; this is an Anki-specific format on top.

**Effort:** M
**Priority:** P3
**Depends on:** lersi export (v1 base)

---

### Daily review reminders

**What:** A way to notify the user daily when they have overdue concepts. Options: (a) cron job calling `lersi due-today` and sending a desktop notification, (b) `lersi daemon` background process, (c) README documentation for setting up a personal cron.

**Why:** Spaced repetition only works if the user actually does the reviews. The `learn__due_today` tool surfaces overdue concepts but only when the user starts a session. Proactive reminders drive retention.

**Context:** `learn__due_today` MCP tool already surfaces overdue reviews. For reminders: simplest v2 approach is a README section showing how to add a cron job (`0 9 * * * lersi due-today --notify`). The `--notify` flag would use `notify-rust` crate for cross-platform desktop notifications. Defer until v1 is stable.

**Effort:** M
**Priority:** P3
**Depends on:** learn__due_today (v1)

---

### lersi debug CLI

**What:** `lersi debug <topic>` dumps the full session history for a topic as formatted JSON — all concepts with their mastery, repetitions, ease_factor, and all session records.

**Why:** When users report weird SM-2 behavior or unexpected mastery scores, this is the first debugging tool. Without it, debugging requires manual SQLite queries.

**Context:** Simple: `SELECT * FROM concepts WHERE topic = ?; SELECT * FROM learning_sessions WHERE concept_id IN (SELECT id FROM concepts WHERE topic = ?);` Dump as pretty JSON. No logic, just data access.

**Effort:** S
**Priority:** P3
**Depends on:** None

---

## Completed

*(nothing yet)*
