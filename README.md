# Lersi

An MCP server for LLM-driven spaced repetition learning. Plug it into any MCP-compatible AI client (Claude, etc.) and the AI manages your learning curriculum using the SM-2 algorithm — scheduling reviews, tracking mastery, and surfacing what's due.

## How it works

1. The AI generates a concept graph for a topic you want to learn.
2. Lersi stores it in a local SQLite database and schedules reviews using SM-2.
3. The AI calls `learn__next_concept` to get what to study next — overdue reviews first, then new concepts in curriculum order, respecting prerequisites.
4. After teaching or quizzing you, it calls `learn__record_review` with a quality score (0–5).
5. SM-2 computes the next interval. Mastery reaches 1.0 after 5 consecutive successful reviews.

## Installation

```bash
cargo install --path .
```

Requires Rust 1.70+. SQLite is bundled — no system dependency needed.

## MCP configuration

Add to your MCP client config (e.g. `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "lersi": {
      "command": "lersi"
    }
  }
}
```

## Tools

| Tool | Description |
|------|-------------|
| `learn__start_topic` | Initialize a topic with a generated curriculum. Existing concepts are preserved (no progress reset). Pass `prior_knowledge` to skip concepts already known. |
| `learn__next_concept` | Get the next concept to study. Returns overdue reviews first, then new concepts in order. Returns `all_done` when everything is mastered, `no_due` when nothing is due today. |
| `learn__record_review` | Record a review outcome using SM-2 quality scores: 0=blackout, 1=wrong, 2=wrong but familiar, 3=correct with difficulty, 4=correct with hesitation, 5=perfect recall. |
| `learn__status` | Get progress for one or all topics: mastered/in-progress/not-started counts and overdue reviews. |

## Database

Lersi stores data in the platform data directory:

- **Linux:** `~/.local/share/lersi/lersi.db`
- **macOS:** `~/Library/Application Support/lersi/lersi.db`

Override with the `LERSI_DB_PATH` environment variable.

## License

MIT
