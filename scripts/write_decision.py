#!/usr/bin/env python3
"""Write an ADR-style decision record for an LTO run."""

from __future__ import annotations

import argparse
import re
import sys
import unicodedata
from datetime import date
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from lto import artifacts as af  # noqa: E402
from lto import state as st  # noqa: E402


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Write an LTO decision ADR")
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--title", required=True)
    parser.add_argument("--context", required=True)
    parser.add_argument("--decision", required=True)
    parser.add_argument("--consequences", required=True)
    parser.add_argument("--status", default="accepted")
    parser.add_argument("--slug")
    parser.add_argument("--memory-slug", dest="memory_slug")
    parser.add_argument("--memory-flow-slug", dest="memory_slug", help=argparse.SUPPRESS)
    return parser.parse_args(argv)


def slugify(raw: str) -> str:
    ascii_text = unicodedata.normalize("NFKD", raw).encode("ascii", "ignore").decode("ascii")
    slug = re.sub(r"[^A-Za-z0-9]+", "-", ascii_text).strip("-").lower()
    return re.sub(r"-{2,}", "-", slug)


def safe_slug(title: str, explicit: str | None) -> str:
    source = explicit if explicit is not None else title
    if explicit is not None and (explicit.startswith(("/", "\\")) or "/" in explicit or "\\" in explicit or ".." in explicit):
        raise ValueError("invalid slug: path traversal or separators are not allowed")
    slug = slugify(source)
    if not slug:
        raise ValueError("slug is empty after normalization; provide --slug")
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,95}", slug):
        raise ValueError("invalid slug: use lowercase ASCII letters, digits, and hyphens")
    return slug


def render_adr(args: argparse.Namespace, run_id: str, slug: str, today: str) -> str:
    lines = [
        f"# {args.title}",
        "",
        f"- status: {args.status}",
        f"- date: {today}",
        f"- lto_run: {run_id}",
        f"- slug: {slug}",
    ]
    if args.memory_slug:
        lines.append(f"- memory_slug: {args.memory_slug}")
    lines.extend([
        "",
        "## Context",
        "",
        args.context.strip(),
        "",
        "## Decision",
        "",
        args.decision.strip(),
        "",
        "## Consequences",
        "",
        args.consequences.strip(),
        "",
    ])
    return "\n".join(lines)


def register_decision(repo: Path, run_id: str, rel_path: str, args: argparse.Namespace, slug: str) -> int:
    state_path = repo / ".lto" / run_id / "state.json"
    if not state_path.exists():
        print(
            f"WARN no state.json for run {run_id}; wrote ADR without LTO registration",
            file=sys.stderr,
        )
        return 0

    state = st.load_state(state_path)
    if state is None:
        print(
            f"WARN cannot load state.json for run {run_id}; wrote ADR without LTO registration",
            file=sys.stderr,
        )
        return 0

    state.setdefault("user_decisions", []).append({
        "title": args.title,
        "status": args.status,
        "path": rel_path,
        "slug": slug,
        "memory_slug": args.memory_slug or "",
        "timestamp": st.iso_now(),
    })
    st.save_state(state_path, state)
    st.sync_run_state_md(repo / ".lto" / run_id / "run-state.md", state)
    af.register_path(
        repo, run_id, repo / rel_path, kind="decision_record",
        producer="write_decision.py", state=state, summary=args.title,
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    repo = args.repo.resolve()
    run_id = st.validate_run_id(args.run_id)
    try:
        slug = safe_slug(args.title, args.slug)
    except ValueError as exc:
        print(f"ERROR {exc}", file=sys.stderr)
        return 2

    today = date.today().isoformat()
    rel_path = f"docs/decisions/{today}-{slug}.md"
    path = repo / rel_path
    if path.exists():
        print(f"ERROR decision already exists: {rel_path}; provide a distinct --slug", file=sys.stderr)
        return 2

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_adr(args, run_id, slug, today), encoding="utf-8")
    try:
        rc = register_decision(repo, run_id, rel_path, args, slug)
    except Exception as exc:
        print(f"ERROR wrote ADR but failed LTO registration: {rel_path}: {exc}", file=sys.stderr)
        return 1

    print(rel_path)
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
