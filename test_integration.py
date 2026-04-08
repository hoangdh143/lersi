#!/usr/bin/env python3
"""
Integration tests for Lersi MCP server + Xiaomi MiMo (Moltis) LLM.

Tests:
  1. MCP protocol layer  — verify all 4 tools work via stdio JSON-RPC
  2. MiMo tool-calling   — verify the LLM emits valid tool calls for lersi tools

Usage:
  python3 test_integration.py          # run all tests
  python3 test_integration.py mcp      # only MCP tests
  python3 test_integration.py mimo     # only MiMo tests
"""

import json
import os
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path
from typing import Any

# ── Load .env ────────────────────────────────────────────────────────────────

def load_env(path: Path) -> None:
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            k, _, v = line.partition("=")
            os.environ.setdefault(k.strip(), v.strip())

ENV_FILE = Path(__file__).parent / ".env"
if ENV_FILE.exists():
    load_env(ENV_FILE)

MIMO_API_KEY = os.environ.get("MIMO_API_KEY", "")
MIMO_BASE_URL = os.environ.get("MIMO_BASE_URL", "https://api.xiaomimimo.com/v1")
MIMO_MODEL = os.environ.get("MIMO_MODEL", "mimo-v2-flash")
LERSI_BIN = Path(__file__).parent / "target" / "release" / "lersi"

# ── Colour helpers ────────────────────────────────────────────────────────────

GREEN  = "\033[1;32m"
RED    = "\033[1;31m"
YELLOW = "\033[1;33m"
RESET  = "\033[0m"

PASS = 0
FAIL = 0

def ok(name: str, detail: str = "") -> None:
    global PASS
    PASS += 1
    extra = f"  {detail}" if detail else ""
    print(f"  {GREEN}PASS{RESET}  {name}{extra}")

def fail(name: str, detail: str = "") -> None:
    global FAIL
    FAIL += 1
    extra = f"\n        {detail}" if detail else ""
    print(f"  {RED}FAIL{RESET}  {name}{extra}")

def section(title: str) -> None:
    print(f"\n{YELLOW}{title}{RESET}")

# ── MCP helpers ───────────────────────────────────────────────────────────────

class MCPSession:
    """Run the lersi binary and communicate over stdio JSON-RPC."""

    def __init__(self, db_path: str) -> None:
        env = {**os.environ, "LERSI_DB_PATH": db_path}
        self._proc = subprocess.Popen(
            [str(LERSI_BIN)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=env,
        )
        self._id = 0

    def call(self, method: str, params: dict | None = None) -> Any:
        self._id += 1
        req = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            req["params"] = params
        line = json.dumps(req) + "\n"
        self._proc.stdin.write(line.encode())
        self._proc.stdin.flush()
        raw = self._proc.stdout.readline()
        return json.loads(raw)

    def tool(self, name: str, args: dict) -> Any:
        return self.call("tools/call", {"name": name, "arguments": args})

    def close(self) -> None:
        self._proc.stdin.close()
        self._proc.wait(timeout=5)


def tool_result(resp: Any) -> Any:
    """Parse the text content from a tools/call response."""
    text = resp["result"]["content"][0]["text"]
    return json.loads(text)

# ── MCP Tests ────────────────────────────────────────────────────────────────

def run_mcp_tests() -> None:
    section("1. MCP protocol tests")

    if not LERSI_BIN.exists():
        fail("binary exists", f"not found at {LERSI_BIN} — run: cargo build --release")
        return

    with tempfile.TemporaryDirectory() as tmpdir:
        db = str(Path(tmpdir) / "lersi.db")
        s = MCPSession(db)

        # ── initialize ──────────────────────────────────────────────────────
        try:
            resp = s.call("initialize")
            info = resp["result"]["serverInfo"]
            assert info["name"] == "lersi", f"unexpected name: {info}"
            ok("initialize", f"server={info['name']} v{info['version']}")
        except Exception as e:
            fail("initialize", str(e))

        # ── tools/list ──────────────────────────────────────────────────────
        try:
            resp = s.call("tools/list")
            tools = resp["result"]["tools"]
            names = {t["name"] for t in tools}
            expected = {
                "learn__start_topic",
                "learn__next_concept",
                "learn__record_review",
                "learn__status",
            }
            assert names == expected, f"got {names}"
            ok("tools/list", f"{len(tools)} tools returned")
        except Exception as e:
            fail("tools/list", str(e))

        # ── learn__status on empty DB ────────────────────────────────────────
        try:
            resp = s.tool("learn__status", {})
            r = tool_result(resp)
            assert r["topics"] == [], f"expected empty topics, got {r}"
            ok("learn__status (empty DB)")
        except Exception as e:
            fail("learn__status (empty DB)", str(e))

        # ── learn__start_topic ───────────────────────────────────────────────
        try:
            resp = s.tool("learn__start_topic", {
                "topic": "Python Basics",
                "concept_graph": {
                    "concepts": [
                        {"title": "Variables", "summary": "Storing values"},
                        {"title": "Functions", "summary": "Reusable code blocks",
                         "prerequisites": ["Variables"]},
                        {"title": "Classes", "summary": "OOP in Python",
                         "prerequisites": ["Functions"]},
                    ]
                },
                "prior_knowledge": ["Variables"],
            })
            r = tool_result(resp)
            assert r["total_concepts"] == 3
            assert r["new_concepts"] == 3   # all inserted fresh; prior_knowledge marks mastered after insert
            assert r["prior_knowledge_marked"] == 1
            ok("learn__start_topic", f"{r['total_concepts']} concepts, {r['prior_knowledge_marked']} pre-mastered")
        except Exception as e:
            fail("learn__start_topic", str(e))

        # ── learn__next_concept ──────────────────────────────────────────────
        try:
            resp = s.tool("learn__next_concept", {"topic": "Python Basics"})
            r = tool_result(resp)
            assert r["status"] == "concept"
            assert r["concept"]["title"] == "Functions"  # Variables mastered → Functions first
            concept_id = r["concept"]["id"]
            ok("learn__next_concept", f"title={r['concept']['title']} id={concept_id}")
        except Exception as e:
            fail("learn__next_concept", str(e))
            concept_id = None

        # ── learn__record_review ─────────────────────────────────────────────
        if concept_id is not None:
            try:
                resp = s.tool("learn__record_review", {"concept_id": concept_id, "quality": 5})
                r = tool_result(resp)
                assert r["passed"] is True
                assert r["quality"] == 5
                ok("learn__record_review (quality=5)", f"mastery={r['mastery']:.2f} next_in={r['next_review_in_days']}d")
            except Exception as e:
                fail("learn__record_review (quality=5)", str(e))

            try:
                resp = s.tool("learn__record_review", {"concept_id": concept_id, "quality": 1})
                r = tool_result(resp)
                assert r["passed"] is False
                ok("learn__record_review (quality=1 — fail)")
            except Exception as e:
                fail("learn__record_review (quality=1)", str(e))

        # ── invalid quality ──────────────────────────────────────────────────
        try:
            resp = s.tool("learn__record_review", {"concept_id": 999, "quality": 9})
            assert resp["result"].get("isError") is True
            ok("learn__record_review rejects quality>5")
        except Exception as e:
            fail("learn__record_review rejects quality>5", str(e))

        # ── unknown topic ────────────────────────────────────────────────────
        try:
            resp = s.tool("learn__next_concept", {"topic": "Nonexistent"})
            assert resp["result"].get("isError") is True
            ok("learn__next_concept unknown topic returns error")
        except Exception as e:
            fail("learn__next_concept unknown topic", str(e))

        # ── idempotent start_topic ───────────────────────────────────────────
        try:
            resp = s.tool("learn__start_topic", {
                "topic": "Python Basics",
                "concept_graph": {"concepts": [{"title": "Variables"}]},
            })
            r = tool_result(resp)
            assert r["existing_concepts"] == 1
            assert r["new_concepts"] == 0
            ok("learn__start_topic idempotent (no progress reset)")
        except Exception as e:
            fail("learn__start_topic idempotent", str(e))

        # ── learn__status after activity ─────────────────────────────────────
        try:
            resp = s.tool("learn__status", {"topic": "Python Basics"})
            r = tool_result(resp)
            stats = r["topics"][0]
            assert stats["total"] == 3
            ok("learn__status after activity", f"mastered={stats['mastered']} in_progress={stats['in_progress']}")
        except Exception as e:
            fail("learn__status after activity", str(e))

        # ── ping ─────────────────────────────────────────────────────────────
        try:
            resp = s.call("ping")
            assert resp["result"] == {}
            ok("ping")
        except Exception as e:
            fail("ping", str(e))

        s.close()

# ── MiMo Tests ────────────────────────────────────────────────────────────────

LERSI_TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "learn__start_topic",
            "description": (
                "Initialize a learning topic with a generated curriculum (ConceptGraph). "
                "Call this at the start of a new learning session for a topic."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "topic": {"type": "string"},
                    "concept_graph": {
                        "type": "object",
                        "properties": {
                            "concepts": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "title":         {"type": "string"},
                                        "summary":       {"type": "string"},
                                        "prerequisites": {"type": "array", "items": {"type": "string"}},
                                    },
                                    "required": ["title"],
                                },
                            }
                        },
                        "required": ["concepts"],
                    },
                    "prior_knowledge": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["topic", "concept_graph"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "learn__next_concept",
            "description": "Get the next concept for the user to study.",
            "parameters": {
                "type": "object",
                "properties": {
                    "topic": {"type": "string"},
                },
                "required": ["topic"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "learn__record_review",
            "description": "Record the outcome of a concept review using SM-2 quality scores (0–5).",
            "parameters": {
                "type": "object",
                "properties": {
                    "concept_id": {"type": "integer"},
                    "quality":    {"type": "integer", "minimum": 0, "maximum": 5},
                },
                "required": ["concept_id", "quality"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "learn__status",
            "description": "Get learning progress for one or all topics.",
            "parameters": {
                "type": "object",
                "properties": {
                    "topic": {"type": "string"},
                },
            },
        },
    },
]


def mimo_chat(messages: list, tools: list | None = None) -> Any:
    """Call the MiMo API (OpenAI-compatible)."""
    try:
        import urllib.request
    except ImportError:
        raise RuntimeError("stdlib urllib not available")

    payload: dict[str, Any] = {
        "model": MIMO_MODEL,
        "messages": messages,
        "max_tokens": 512,
    }
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "auto"

    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        f"{MIMO_BASE_URL}/chat/completions",
        data=data,
        headers={
            "Authorization": f"Bearer {MIMO_API_KEY}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def run_mimo_tests() -> None:
    section("2. MiMo (Moltis LLM) tool-calling tests")

    if not MIMO_API_KEY:
        fail("API key present", "MIMO_API_KEY not set in .env — skipping MiMo tests")
        return

    # ── basic connectivity ───────────────────────────────────────────────────
    try:
        resp = mimo_chat([{"role": "user", "content": "Reply with exactly: pong"}])
        text = resp["choices"][0]["message"]["content"] or ""
        assert "pong" in text.lower(), f"unexpected reply: {text!r}"
        ok("MiMo API reachable", f"model={resp.get('model', MIMO_MODEL)}")
    except Exception as e:
        fail("MiMo API reachable", str(e))
        return  # no point running further tests if API is down

    # ── tool call: start_topic ───────────────────────────────────────────────
    try:
        resp = mimo_chat(
            messages=[{
                "role": "user",
                "content": (
                    "I want to learn Rust. "
                    "Call learn__start_topic with a short 3-concept curriculum."
                ),
            }],
            tools=LERSI_TOOLS,
        )
        msg = resp["choices"][0]["message"]
        calls = msg.get("tool_calls") or []
        assert calls, f"no tool_calls in response — message: {msg.get('content')!r}"
        call = calls[0]["function"]
        assert call["name"] == "learn__start_topic", f"called {call['name']!r} instead"
        args = json.loads(call["arguments"])
        assert "topic" in args, f"missing 'topic' in args: {args}"
        assert "concept_graph" in args, f"missing 'concept_graph': {args}"
        concepts = args["concept_graph"]["concepts"]
        assert len(concepts) >= 2, f"expected ≥2 concepts, got {concepts}"
        ok("MiMo calls learn__start_topic",
           f"topic={args['topic']!r} concepts={len(concepts)}")
    except Exception as e:
        fail("MiMo calls learn__start_topic", str(e))

    # ── tool call: next_concept ──────────────────────────────────────────────
    try:
        resp = mimo_chat(
            messages=[{
                "role": "user",
                "content": "What should I study next for my Rust topic?",
            }],
            tools=LERSI_TOOLS,
        )
        msg = resp["choices"][0]["message"]
        calls = msg.get("tool_calls") or []
        assert calls, f"no tool_calls — content: {msg.get('content')!r}"
        call = calls[0]["function"]
        assert call["name"] == "learn__next_concept", f"called {call['name']!r}"
        args = json.loads(call["arguments"])
        assert "topic" in args, f"missing topic: {args}"
        ok("MiMo calls learn__next_concept", f"topic={args['topic']!r}")
    except Exception as e:
        fail("MiMo calls learn__next_concept", str(e))

    # ── tool call: record_review ─────────────────────────────────────────────
    try:
        resp = mimo_chat(
            messages=[
                {"role": "user", "content": "I just reviewed concept #42 and got it perfectly."},
            ],
            tools=LERSI_TOOLS,
        )
        msg = resp["choices"][0]["message"]
        calls = msg.get("tool_calls") or []
        assert calls, f"no tool_calls — content: {msg.get('content')!r}"
        call = calls[0]["function"]
        assert call["name"] == "learn__record_review", f"called {call['name']!r}"
        args = json.loads(call["arguments"])
        assert "concept_id" in args, f"missing concept_id: {args}"
        assert "quality" in args, f"missing quality: {args}"
        assert 0 <= args["quality"] <= 5, f"quality out of range: {args['quality']}"
        assert args["concept_id"] == 42, f"expected id=42, got {args['concept_id']}"
        ok("MiMo calls learn__record_review",
           f"concept_id={args['concept_id']} quality={args['quality']}")
    except Exception as e:
        fail("MiMo calls learn__record_review", str(e))

    # ── tool call: status ────────────────────────────────────────────────────
    try:
        resp = mimo_chat(
            messages=[{"role": "user", "content": "Show me my overall learning progress."}],
            tools=LERSI_TOOLS,
        )
        msg = resp["choices"][0]["message"]
        calls = msg.get("tool_calls") or []
        assert calls, f"no tool_calls — content: {msg.get('content')!r}"
        call = calls[0]["function"]
        assert call["name"] == "learn__status", f"called {call['name']!r}"
        ok("MiMo calls learn__status")
    except Exception as e:
        fail("MiMo calls learn__status", str(e))

    # ── multi-turn: start → next → record ────────────────────────────────────
    try:
        # Turn 1: start topic
        resp1 = mimo_chat(
            messages=[{
                "role": "user",
                "content": "Start a topic called 'SQL Basics' with 2 concepts.",
            }],
            tools=LERSI_TOOLS,
        )
        msg1 = resp1["choices"][0]["message"]
        assert msg1.get("tool_calls"), "expected tool call on turn 1"
        fn1 = msg1["tool_calls"][0]["function"]["name"]
        assert fn1 == "learn__start_topic", f"turn 1: expected start_topic, got {fn1}"

        # Simulate the tool result coming back, ask for next concept
        tc = msg1["tool_calls"][0]
        resp2 = mimo_chat(
            messages=[
                {"role": "user", "content": "Start a topic called 'SQL Basics' with 2 concepts."},
                {"role": "assistant", "content": None, "tool_calls": [tc]},
                {"role": "tool", "tool_call_id": tc["id"],
                 "content": json.dumps({"topic": "SQL Basics", "total_concepts": 2,
                                        "new_concepts": 2, "existing_concepts": 0,
                                        "message": "Ready! 2 concepts loaded."})},
                {"role": "user", "content": "Good. What should I study first?"},
            ],
            tools=LERSI_TOOLS,
        )
        msg2 = resp2["choices"][0]["message"]
        assert msg2.get("tool_calls"), "expected tool call on turn 2"
        fn2 = msg2["tool_calls"][0]["function"]["name"]
        assert fn2 == "learn__next_concept", f"turn 2: expected next_concept, got {fn2}"

        ok("MiMo multi-turn: start → next_concept")
    except Exception as e:
        fail("MiMo multi-turn", str(e))

# ── Entry point ───────────────────────────────────────────────────────────────

def main() -> None:
    mode = sys.argv[1].lower() if len(sys.argv) > 1 else "all"

    if mode in ("all", "mcp"):
        run_mcp_tests()
    if mode in ("all", "mimo"):
        run_mimo_tests()

    total = PASS + FAIL
    print(f"\n{'─'*50}")
    if FAIL == 0:
        print(f"{GREEN}All {total} tests passed.{RESET}")
    else:
        print(f"{RED}{FAIL}/{total} tests failed.{RESET}")
    sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
