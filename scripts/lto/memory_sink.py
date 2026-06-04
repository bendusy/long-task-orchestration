"""Memory sink implementations for LTO projection publishing/resume."""

from __future__ import annotations

import json
import os
import hashlib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any


class MemorySinkError(RuntimeError):
    """Raised when a configured memory sink cannot complete a request."""


@dataclass
class PublishResult:
    ok: bool
    published: int
    detail: str


@dataclass
class ResumeResult:
    ok: bool
    detail: str
    payload: dict[str, Any] | None = None


class MemorySink:
    def publish(self, payload: dict[str, Any]) -> PublishResult:
        raise NotImplementedError

    def resume(self, project_key: str) -> ResumeResult:
        raise NotImplementedError


class LegacyMemoryFlowSink(MemorySink):
    """Optional REST adapter for memory-flow compatible write/resume APIs."""

    def __init__(self, url: str | None = None, token: str | None = None, timeout: float = 5.0):
        self.url = (url or os.getenv("MEMORY_FLOW_URL") or "").rstrip("/")
        self.token = token or os.getenv("MEMORY_FLOW_TOKEN") or ""
        self.timeout = timeout

    def publish(self, payload: dict[str, Any]) -> PublishResult:
        self._require_config(require_token=True)
        records = list(payload.get("records", []) or [])
        published = 0
        for record in records:
            if record.get("kind") == "workflow_routing_memory":
                continue
            self._post_write(_record_to_experience(payload, record))
            published += 1
        return PublishResult(True, published, f"published {published} records")

    def resume(self, project_key: str) -> ResumeResult:
        self._require_config(require_token=False)
        query = urllib.parse.urlencode({
            "q": f"lto {project_key} project_snapshot lto_run_snapshot",
            "library": os.getenv("LTO_MEMORY_LIBRARY", "tech"),
            "top_k": "5",
        })
        req = urllib.request.Request(f"{self.url}/v1/search?{query}", headers=self._headers())
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                body = resp.read().decode("utf-8", errors="replace")
        except (urllib.error.URLError, TimeoutError) as exc:
            raise MemorySinkError(str(exc)) from exc
        try:
            parsed = json.loads(body)
        except json.JSONDecodeError:
            parsed = {"raw": body}
        return ResumeResult(True, "memory sink search ok", parsed)

    def _require_config(self, *, require_token: bool) -> None:
        if not self.url:
            raise MemorySinkError(
                "optional memory sink is not configured "
                "(set MEMORY_FLOW_URL or pass --url; LTO core commands do not require a memory sink)"
            )
        if require_token and not self.token:
            raise MemorySinkError(
                "MEMORY_FLOW_TOKEN is not configured for optional memory publish "
                "(or pass --token)"
            )

    def _headers(self) -> dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        if agent := os.getenv("MEMORY_FLOW_AGENT_ID"):
            headers["X-Agent-ID"] = agent
        return headers

    def _post_write(self, body: dict[str, Any]) -> None:
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        req = urllib.request.Request(
            f"{self.url}/v1/write", data=data, headers=self._headers(), method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                if resp.status not in (200, 201):
                    raise MemorySinkError(f"memory sink write returned {resp.status}")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")[:500]
            raise MemorySinkError(f"memory sink write returned {exc.code}: {detail}") from exc
        except (urllib.error.URLError, TimeoutError) as exc:
            raise MemorySinkError(str(exc)) from exc


def _record_to_experience(payload: dict[str, Any], record: dict[str, Any]) -> dict[str, Any]:
    project = payload.get("project_key", "unknown")
    run_id = payload.get("run_id", "unknown")
    kind = record.get("kind", "record")
    suffix = _stable_suffix(record)
    title = f"LTO {project}/{run_id} {kind}"
    body = (
        "[要点]\n"
        f"project={project}\nrun_id={run_id}\nkind={kind}\n"
        f"record={json.dumps(record, ensure_ascii=False, sort_keys=True)}\n\n"
        "[范式]\n"
        "This is a redacted LTO artifact-memory projection. Local .lto remains the source of truth."
    )
    return {
        "slug": f"lto-{project}-{run_id}-{kind}-{suffix}",
        "library": os.getenv("LTO_MEMORY_LIBRARY", "tech"),
        "type_": os.getenv("LTO_MEMORY_TYPE", "experience"),
        "title": title[:120],
        "body": body,
        "file_path": record.get("relative_path") or record.get("state_path") or f".lto/{run_id}/state.json",
        "tags": ["lto", "artifact-memory", kind],
        "task_id": record.get("task_id") or "",
        "source_agent": payload.get("host_runtime") or "lto",
    }


def _stable_suffix(record: dict[str, Any]) -> str:
    seed = json.dumps(record, ensure_ascii=False, sort_keys=True)
    return hashlib.sha1(seed.encode("utf-8")).hexdigest()[:10]


def print_resume_result(result: ResumeResult) -> None:
    print("=== LTO MEMORY RESUME ===")
    print(result.detail)
    if result.payload is not None:
        print(json.dumps(result.payload, ensure_ascii=False, indent=2, sort_keys=True)[:6000])
    print("=========================")
