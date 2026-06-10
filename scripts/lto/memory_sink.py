"""Memory sink implementations for LTO projection publishing/resume."""

from __future__ import annotations

import json
import os
import hashlib
import shutil
import subprocess
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
    # three-state breakdown when the sink reports it (am ingest); legacy REST
    # leaves these at zero and only sets `published`.
    written: int = 0
    updated: int = 0
    skipped: int = 0
    failed: int = 0


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
    """Temporary private REST adapter for legacy memory-flow write/resume APIs."""

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
            "library": "技术",
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
        return ResumeResult(True, "legacy memory-flow search ok", parsed)

    def _require_config(self, *, require_token: bool) -> None:
        if not self.url:
            raise MemorySinkError(
                "optional memory sink is not configured "
                "(set MEMORY_FLOW_URL or pass --url; LTO core commands do not require ANIMEM)"
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
                    raise MemorySinkError(f"memory-flow write returned {resp.status}")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")[:500]
            raise MemorySinkError(f"memory-flow write returned {exc.code}: {detail}") from exc
        except (urllib.error.URLError, TimeoutError) as exc:
            raise MemorySinkError(str(exc)) from exc


class AmCliSink(MemorySink):
    """Native am-CLI adapter: pipes the LTO projection envelope into `am ingest`.

    The recommended path since am 0.7.0. The whole `memory export` envelope is
    handed to `am ingest -f - --json` on stdin verbatim — am owns slug
    generation, three-state dedup (written/updated/skipped) and supersede
    versioning. LTO never constructs slugs or touches PG.

    Security: we deliberately do NOT pass --database-url. am reads the
    connection string from its own env/default, so no PG credential ever
    enters LTO's process args, logs, or repo.
    """

    def __init__(self, binary: str | None = None, timeout: float = 60.0):
        self.binary = binary or os.getenv("AM_BIN") or "am"
        self.timeout = timeout

    def publish(self, payload: dict[str, Any]) -> PublishResult:
        self._require_binary()
        envelope = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        try:
            proc = subprocess.run(
                [self.binary, "ingest", "-f", "-", "--json"],
                input=envelope,
                capture_output=True,
                timeout=self.timeout,
            )
        except subprocess.TimeoutExpired as exc:
            raise MemorySinkError(f"am ingest timed out after {self.timeout}s") from exc
        except OSError as exc:
            raise MemorySinkError(f"am ingest could not run: {exc}") from exc
        stdout = proc.stdout.decode("utf-8", errors="replace")
        stderr = proc.stderr.decode("utf-8", errors="replace")
        if proc.returncode != 0:
            raise MemorySinkError(
                f"am ingest exited {proc.returncode}: {(stderr or stdout)[:500]}"
            )
        return self._parse_ingest_output(stdout, stderr)

    def resume(self, project_key: str) -> ResumeResult:
        self._require_binary()
        query = f"lto {project_key} project_snapshot lto_run_snapshot"
        try:
            proc = subprocess.run(
                [self.binary, "search", query, "--library", "技术",
                 "--json", "--top-k", "5"],
                capture_output=True,
                timeout=self.timeout,
            )
        except subprocess.TimeoutExpired as exc:
            raise MemorySinkError(f"am search timed out after {self.timeout}s") from exc
        except OSError as exc:
            raise MemorySinkError(f"am search could not run: {exc}") from exc
        stdout = proc.stdout.decode("utf-8", errors="replace")
        stderr = proc.stderr.decode("utf-8", errors="replace")
        if proc.returncode != 0:
            raise MemorySinkError(
                f"am search exited {proc.returncode}: {(stderr or stdout)[:500]}"
            )
        try:
            parsed = json.loads(stdout)
        except json.JSONDecodeError:
            parsed = {"raw": stdout[:6000]}
        return ResumeResult(True, "am search ok", parsed)

    def _require_binary(self) -> None:
        if shutil.which(self.binary) is None:
            raise MemorySinkError(
                f"am CLI not found on PATH (looked for '{self.binary}'); "
                "install am or set AM_BIN. LTO core commands do not require am — "
                "local .lto/ remains the source of truth."
            )

    @staticmethod
    def _parse_ingest_output(stdout: str, stderr: str) -> PublishResult:
        try:
            doc = json.loads(stdout)
        except json.JSONDecodeError as exc:
            raise MemorySinkError(
                f"am ingest produced non-JSON output: {(stdout or stderr)[:500]}"
            ) from exc
        if not doc.get("ok", False):
            raise MemorySinkError(f"am ingest reported failure: {json.dumps(doc)[:500]}")
        summary = (doc.get("data") or {}).get("summary") or {}
        written = int(summary.get("written", 0))
        updated = int(summary.get("updated", 0))
        skipped = int(summary.get("skipped", 0))
        failed = int(summary.get("failed", 0))
        published = written + updated
        detail = (
            f"am ingest: {written} written, {updated} updated, "
            f"{skipped} skipped, {failed} failed"
        )
        return PublishResult(
            ok=failed == 0,
            published=published,
            detail=detail,
            written=written,
            updated=updated,
            skipped=skipped,
            failed=failed,
        )


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
        "library": "技术",
        "type_": "经验",
        "title": title[:120],
        "body": body,
        "file_path": record.get("relative_path") or record.get("state_path") or f".lto/{run_id}/state.json",
        "tags": ["lto", "animem", "artifact-memory", kind],
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
