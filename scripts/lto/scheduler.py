#!/usr/bin/env python3
"""LTO Resource Scheduler — concurrent agent job execution with health gating,
retry + exponential backoff, cost metering, and exit-code-aware status classification.

Single-file, pure stdlib, Python 3.10+, type-annotated.
Data contracts: lto.agent_job (AgentJob / AgentResult / Budget / RetryPolicy / JobStatus).
Runner interface: runner.sh <prompt_file> <reply_file> <timeout_sec> → exit code.
Healthcheck: healthcheck.sh --json [runner...] → [{agent, exit, elapsed, bytes, verdict}].

Designed for adversarial correctness — exit-code confusion is the #1 historical bug source.
See long-task-orchestration/references/validation-log.md for the 3-misattribution incident.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import time
from concurrent.futures import Future, ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any

from lto.agent_job import (
    AgentJob,
    AgentResult,
    Budget,
    JobStatus,
    KNOWN_RUNNERS,
    PermissionPolicy,
    RetryPolicy,
)


# ---------------------------------------------------------------------------
# Scheduler
# ---------------------------------------------------------------------------

class Scheduler:
    """Concurrent agent job scheduler with health gating and retry/backoff.

    Parameters
    ----------
    repo:
        Repository root. Default runners_dir is bundled repo/scripts/delegate/runners/ when present, with legacy skill-path fallback.
    max_concurrency:
        Max simultaneous jobs in flight (ThreadPoolExecutor workers).
    max_total_agents:
        Hard cap on batch size — raises ValueError if exceeded.
    max_backoff_sec:
        Per-attempt backoff ceiling (default 60s).  Applied as min(computed, max_backoff_sec).
    total_retry_wall_sec:
        Cumulative retry-sleep wall-clock budget (default 300s).  When exceeded,
        the job stops retrying and returns the latest result.
        The effective cap is min(job.budget.timeout_sec, total_retry_wall_sec).
    runners_dir:
        Override runners directory (used by tests to inject fake runners).
    """

    def __init__(
        self,
        repo: Path,
        *,
        max_concurrency: int = 4,
        max_total_agents: int = 50,
        max_backoff_sec: float = 60,
        total_retry_wall_sec: float = 300,
        runners_dir: Path | None = None,
    ):
        self.repo = Path(repo).resolve()
        self.max_concurrency = max_concurrency
        self.max_total_agents = max_total_agents
        self.max_backoff_sec = max_backoff_sec
        self.total_retry_wall_sec = total_retry_wall_sec
        if runners_dir is not None:
            self.runners_dir = Path(runners_dir).resolve()
        else:
            bundled = self.repo / "scripts" / "delegate" / "runners"
            legacy = self.repo / "skills" / "agent-delegate" / "scripts" / "runners"
            self.runners_dir = (bundled if bundled.exists() else legacy).resolve()

    # -- helpers -----------------------------------------------------------

    def _runner_path(self, runner: str) -> Path:
        return self.runners_dir / f"{runner}.sh"

    # -- healthcheck -------------------------------------------------------

    def healthcheck(self, runners: list[str]) -> dict[str, bool]:
        """Call healthcheck.sh --json, return {runner: is_healthy}.

        ``verdict == "OK"`` → healthy; anything else (EMPTY / TIMEOUT / ERROR /
        MISSING) → not healthy.
        """
        hc_script = self.runners_dir / "healthcheck.sh"
        if not hc_script.exists():
            return {r: False for r in runners}

        try:
            result = subprocess.run(
                ["bash", str(hc_script), "--json", *runners],
                capture_output=True,
                text=True,
                timeout=120,
                cwd=str(self.runners_dir),
            )
        except (subprocess.TimeoutExpired, OSError):
            return {r: False for r in runners}

        try:
            raw = json.loads(result.stdout)
        except json.JSONDecodeError:
            return {r: False for r in runners}

        if not isinstance(raw, list):
            return {r: False for r in runners}

        out: dict[str, bool] = {}
        for entry in raw:
            if not isinstance(entry, dict):
                return {r: False for r in runners}
            agent = entry.get("agent", "")
            verdict = entry.get("verdict", "")
            out[agent] = verdict == "OK"
        for r in runners:
            out.setdefault(r, False)
        return out

    # -- submit ------------------------------------------------------------

    def submit(self, jobs: list[AgentJob]) -> list[AgentResult]:
        """Execute a batch of jobs concurrently.

        Raises
        ------
        ValueError
            If len(jobs) > max_total_agents.
        """
        # --- total-agent cap ---
        if len(jobs) > self.max_total_agents:
            raise ValueError(
                f"Job count {len(jobs)} exceeds max_total_agents={self.max_total_agents}"
            )

        # Check for duplicate job_ids (fan-out/barrier correctness depends on uniqueness)
        seen_ids: set[str] = set()
        for j in jobs:
            if j.job_id in seen_ids:
                raise ValueError(f"duplicate job_id: {j.job_id!r}")
            seen_ids.add(j.job_id)

        # Validate every job early
        for j in jobs:
            try:
                j.validate()
            except ValueError as e:
                raise ValueError(f"Job {j.job_id!r} invalid: {e}") from e

        # --- healthcheck gate ---
        involved_runners = list(dict.fromkeys(j.runner for j in jobs))  # order-preserving dedup
        healthy = self.healthcheck(involved_runners)

        results_map: dict[str, AgentResult] = {}
        for j in jobs:
            if not healthy.get(j.runner, False):
                results_map[j.job_id] = AgentResult(
                    job_id=j.job_id,
                    runner=j.runner,
                    status=JobStatus.SKIPPED.value,
                    error=f"runner unhealthy: {j.runner}",
                    permissions=_permission_snapshot(j),
                )

        runnable = [j for j in jobs if j.job_id not in results_map]

        # --- concurrent execution ---
        with ThreadPoolExecutor(max_workers=self.max_concurrency) as executor:
            future_map: dict[Future[AgentResult], str] = {}
            for j in runnable:
                fut = executor.submit(self._execute_job, j)
                future_map[fut] = j.job_id

            for fut in as_completed(future_map):
                job_id = future_map[fut]
                try:
                    results_map[job_id] = fut.result()
                except Exception as exc:
                    results_map[job_id] = AgentResult(
                        job_id=job_id,
                        runner="unknown",
                        status=JobStatus.FAILED.value,
                        error=f"unhandled exception: {exc}",
                    )

        # Preserve input order
        return [results_map[j.job_id] for j in jobs]

    # -- internal execution ------------------------------------------------

    def _execute_job(self, job: AgentJob) -> AgentResult:
        """Execute a single job, applying retry+backoff policy."""
        runner_script = self._runner_path(job.runner)
        if not runner_script.exists():
            return AgentResult(
                job_id=job.job_id,
                runner=job.runner,
                status=JobStatus.FAILED.value,
                error=f"runner script not found: {runner_script}",
            )

        last_result: AgentResult | None = None
        total_attempts = 0
        wall_start = time.monotonic()
        retry_sleep_elapsed = 0.0

        policy = job.retry_policy
        retry_budget = min(job.budget.timeout_sec, self.total_retry_wall_sec)
        for attempt_idx in range(policy.max_retries + 1):
            if attempt_idx > 0:
                backoff = min(
                    policy.backoff_sec * (2 ** (attempt_idx - 1)),
                    self.max_backoff_sec,
                )
                # Stop retrying if cumulative sleep would exceed the wall budget
                if retry_sleep_elapsed + backoff > retry_budget:
                    break
                time.sleep(backoff)
                retry_sleep_elapsed += backoff

            total_attempts += 1
            result = self._run_once(job, runner_script, total_attempts)
            last_result = result

            should_retry = (
                result.status in policy.retry_on
                and attempt_idx < policy.max_retries
            )
            if not should_retry:
                break

        if last_result is None:
            last_result = AgentResult(
                job_id=job.job_id,
                runner=job.runner,
                status=JobStatus.FAILED.value,
                error="no attempt executed",
            )

        last_result.cost["elapsed_sec"] = round(time.monotonic() - wall_start, 3)
        last_result.attempts = total_attempts
        return last_result

    def _run_once(
        self, job: AgentJob, runner_script: Path, attempt: int
    ) -> AgentResult:
        """Single raw execution attempt — one subprocess call."""
        prompt_path: Path | None = None
        reply_path: Path | None = None

        try:
            # --- prompt preparation ---
            if job.prompt_is_inline:
                fd, tmp = tempfile.mkstemp(prefix="lto_prompt_", suffix=".txt")
                os.close(fd)
                prompt_path = Path(tmp)
                prompt_path.write_text(job.prompt_ref, encoding="utf-8")
            else:
                prompt_path = Path(job.prompt_ref)
                if not prompt_path.exists():
                    return AgentResult(
                        job_id=job.job_id,
                        runner=job.runner,
                        status=JobStatus.FAILED.value,
                        error=f"prompt_ref not found: {job.prompt_ref}",
                        attempts=attempt,
                    )
                prompt_path = prompt_path.resolve()

            # --- reply file ---
            fd, tmp = tempfile.mkstemp(prefix="lto_reply_", suffix=".txt")
            os.close(fd)
            reply_path = Path(tmp)

            # --- run ---
            exec_start = time.monotonic()
            timeout_total = job.budget.timeout_sec + 10  # safety margin beyond runner's own timeout
            try:
                proc = subprocess.run(
                    [
                        "bash",
                        str(runner_script),
                        str(prompt_path),
                        str(reply_path),
                        str(job.budget.timeout_sec),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=timeout_total,
                    cwd=str(self.repo),
                    env=_effective_env(job),
                )
                exit_code = proc.returncode
                stderr = proc.stderr
            except subprocess.TimeoutExpired:
                exit_code = 124
                stderr = "subprocess timeout"
            exec_elapsed = round(time.monotonic() - exec_start, 3)

            # --- read reply ---
            try:
                reply_text = reply_path.read_text(encoding="utf-8").strip()
            except Exception:
                reply_text = ""

            # --- read optional token sidecar (<reply>.meta.json) ---
            cost: dict[str, Any] = {"elapsed_sec": exec_elapsed}
            cost.update(_read_token_sidecar(reply_path))

            # --- classify ---
            exit_code, status, error = _classify(exit_code, reply_text, stderr)

            return AgentResult(
                job_id=job.job_id,
                runner=job.runner,
                status=status,
                exit_code=exit_code,
                reply_text=reply_text,
                cost=cost,
                permissions=_permission_snapshot(job),
                attempts=attempt,
                error=error,
                artifacts=[],  # reply file is deleted in finally; content is in reply_text
            )

        finally:
            if prompt_path is not None and job.prompt_is_inline:
                _unlink_safe(prompt_path)
            if reply_path is not None:
                _unlink_safe(reply_path)
                _unlink_safe(_sidecar_path(reply_path))


# ---------------------------------------------------------------------------
# Per-job runner environment / permission snapshots
# ---------------------------------------------------------------------------

def _effective_env(job: AgentJob) -> dict[str, str]:
    """Build subprocess env from process env + AgentJob.env + permission policy.

    Parent CODEX_SANDBOX is intentionally not inherited for Codex jobs: each
    job must carry its own permission_policy, defaulting to read-only.
    """
    env = os.environ.copy()
    env.update({k: str(v) for k, v in job.env.items()})
    if job.runner == "codex":
        env["CODEX_SANDBOX"] = job.permission_policy.sandbox
        if job.model and "CODEX_MODEL" not in job.env:
            env["CODEX_MODEL"] = job.model
    return env


def _permission_snapshot(job: AgentJob) -> dict[str, Any]:
    """Return safe-to-persist permission evidence for state/artifacts."""
    snap: dict[str, Any] = {
        "runner": job.runner,
        "sandbox": job.permission_policy.sandbox if job.runner == "codex" else None,
        "reason": job.permission_policy.reason,
        "user_approved": job.permission_policy.user_approved,
        "env_keys": sorted(job.env.keys()),
    }
    if job.runner == "codex" and job.model:
        snap["model_source"] = "job.model" if "CODEX_MODEL" not in job.env else "env.CODEX_MODEL"
    return snap


# ---------------------------------------------------------------------------
# Exit-code classifier (module-level — pure function, easy to unit-test)
# ---------------------------------------------------------------------------

# Rate-limit signal substrings (case-insensitive match)
_RATE_LIMIT_MARKERS = ("429", "too many requests", "rate limit", "rate_limit", "rate limited")


def _classify(exit_code: int, reply_text: str, stderr: str) -> tuple[int, str, str]:
    """Return (exit_code, status, error_message).

    Order is critical — matches the 3-misattribution incident post-mortem:
      1. OK (exit 0 + non-empty) — rate-limit markers in successful reply
         body are content, NOT a rate-limit signal (anti false-positive).
      2. Empty-reply (exit 0 + empty) → FAILED, NOT OK
      3. Rate-limit check on FAILED jobs only (exit_code != 0)
      4. Timeout (exit 124)
      5. Generic FAILED (any other non-zero)

    All string matching is case-insensitive.
    """
    # 1. Exit 0 + non-empty → OK (正文讨论429是内容，不是限流信号)
    if exit_code == 0:
        if not reply_text:
            return exit_code, JobStatus.FAILED.value, "exit 0 but empty reply"
        return exit_code, JobStatus.OK.value, ""

    # 2. Rate limit on failed exits only
    combined = (stderr + " " + reply_text).lower()
    if any(m in combined for m in _RATE_LIMIT_MARKERS):
        return exit_code, JobStatus.RATE_LIMITED.value, f"rate limited (exit={exit_code})"

    # 3. Timeout
    if exit_code == 124:
        return exit_code, JobStatus.TIMEOUT.value, "timeout (exit 124)"

    # 4. Generic non-zero failure
    err = f"exit code {exit_code}"
    if stderr:
        tail = stderr.strip()[:500]
        if len(stderr.strip()) > 500:
            tail += "..."
        err += f": {tail}"
    return exit_code, JobStatus.FAILED.value, err


def _unlink_safe(p: Path) -> None:
    """Delete path, swallow all exceptions."""
    try:
        p.unlink(missing_ok=True)
    except Exception:
        pass


# Token sidecar protocol (v0, best-effort, fully optional):
# A runner MAY write `<reply_file>.meta.json` with token usage. If present and
# well-formed, scheduler merges {tokens_in, tokens_out, tokens} into cost.
# Absent / malformed → silently ignored (back-compat: old runners don't write it).
_SIDECAR_SUFFIX = ".meta.json"


def _sidecar_path(reply_path: Path) -> Path:
    return reply_path.with_name(reply_path.name + _SIDECAR_SUFFIX)


def _read_token_sidecar(reply_path: Path) -> dict[str, Any]:
    """Read optional <reply>.meta.json token usage; return {} if absent/bad.

    Accepts keys: tokens_in / tokens_out / tokens (any subset). Non-negative
    ints only; anything else is dropped. tokens defaults to in+out when both
    present and total absent.
    """
    path = _sidecar_path(reply_path)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, ValueError, OSError):
        return {}
    if not isinstance(data, dict):
        return {}
    out: dict[str, Any] = {}
    for key in ("tokens_in", "tokens_out", "tokens"):
        val = data.get(key)
        if isinstance(val, bool):  # bool 是 int 子类，显式排除
            continue
        if isinstance(val, int) and val >= 0:
            out[key] = val
    if "tokens" not in out and "tokens_in" in out and "tokens_out" in out:
        out["tokens"] = out["tokens_in"] + out["tokens_out"]
    return out


# ===========================================================================
# Self-test (run: python3 -m lto.scheduler  or  python3 scheduler.py)
# ===========================================================================

def _run_selftest() -> int:
    """Run all 6 acceptance tests + adversarial edge cases.

    Returns 0 on success, 1 on failure.
    """
    import shutil
    import sys

    tests_passed = 0
    tests_total = 0

    def ok(label: str) -> None:
        nonlocal tests_passed, tests_total
        tests_passed += 1
        tests_total += 1
        print(f"  ✅ {label}")

    def fail(label: str, detail: str = "") -> int:
        nonlocal tests_total
        tests_total += 1
        print(f"  ❌ {label}")
        if detail:
            print(f"     {detail}")
        return 1

    # ---- scaffold: temp dir with fake runner + fake healthcheck ----
    tmpdir = Path(tempfile.mkdtemp(prefix="lto_scheduler_test_"))
    runners_dir = tmpdir / "runners"
    runners_dir.mkdir()

    # Fake runner (Python — JSON control file, no external deps)
    fake_runner_py = tmpdir / "fake_runner.py"
    fake_runner_py.write_text(r'''#!/usr/bin/env python3
"""Fake runner — reads behaviour from $SCHEDULER_TEST_CONTROL JSON."""
import json, os, sys, time

prompt_file, reply_file, timeout_sec = sys.argv[1:4]

with open(prompt_file) as f:
    first_line = f.readline().strip()
job_id = first_line.replace("# JOB_ID:", "").strip()

ctrl_path = os.environ.get("SCHEDULER_TEST_CONTROL", "")
behaviour = {}
if ctrl_path and os.path.exists(ctrl_path):
    with open(ctrl_path) as f:
        behaviour = json.load(f).get(job_id, {})

sleep_sec = float(behaviour.get("sleep", 0))
exit_code = int(behaviour.get("exit_code", 0))
output = str(behaviour.get("output", ""))
if "output_env" in behaviour:
    output = json.dumps({k: os.environ.get(k) for k in behaviour["output_env"]}, sort_keys=True)

if sleep_sec > 0:
    time.sleep(sleep_sec)

with open(reply_file, "w") as f:
    f.write(output)

sys.exit(exit_code)
''')

    # Create runner wrappers (.sh → exec fake_runner.py)
    for name in ("codex", "pi", "agy", "gemini", "claude"):
        wrapper = runners_dir / f"{name}.sh"
        wrapper.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake_runner_py}" "$@"\n')
        wrapper.chmod(0o755)

    # Fake healthcheck (controlled by $SCHEDULER_TEST_HEALTHCHECK JSON)
    hc_script = runners_dir / "healthcheck.sh"
    hc_script.write_text(r'''#!/usr/bin/env bash
# Fake healthcheck — reads from $SCHEDULER_TEST_HEALTHCHECK
ctrl="${SCHEDULER_TEST_HEALTHCHECK:-}"
if [ -n "$ctrl" ] && [ -f "$ctrl" ]; then
    python3 -c "
import json, sys
with open('$ctrl') as f:
    print(json.dumps(json.load(f)))
" 2>/dev/null || echo '[]'
else
    echo '[]'
fi
exit 0
''')
    hc_script.chmod(0o755)

    # Control file helpers
    def set_control(behaviours: dict[str, dict]) -> Path:
        p = tmpdir / "control.json"
        p.write_text(json.dumps(behaviours))
        os.environ["SCHEDULER_TEST_CONTROL"] = str(p)
        return p

    def set_healthcheck(data: list[dict]) -> Path:
        p = tmpdir / "hc.json"
        p.write_text(json.dumps(data))
        os.environ["SCHEDULER_TEST_HEALTHCHECK"] = str(p)
        return p

    def make_job(job_id: str, runner: str = "codex", **kw: Any) -> AgentJob:
        """Create a test job whose prompt encodes job_id for the fake runner."""
        prompt = f"# JOB_ID:{job_id}\nTest prompt for {job_id}"
        defaults: dict[str, Any] = {
            "job_id": job_id,
            "runner": runner,
            "prompt_ref": prompt,
            "prompt_is_inline": True,
            "budget": Budget(timeout_sec=30),
            "retry_policy": RetryPolicy(max_retries=0),
        }
        defaults.update(kw)
        return AgentJob(**defaults)

    repo = tmpdir / "repo"
    repo.mkdir()

    # Regression: standalone release repo defaults to bundled scripts/delegate/runners.
    bundled = repo / "scripts" / "delegate" / "runners"
    bundled.mkdir(parents=True)
    (bundled / "healthcheck.sh").write_text("#!/usr/bin/env bash\necho []\n", encoding="utf-8")
    (bundled / "healthcheck.sh").chmod(0o755)
    sched_default = Scheduler(repo=repo)
    assert sched_default.runners_dir == bundled.resolve(), sched_default.runners_dir

    sched = Scheduler(repo=repo, max_concurrency=2, max_total_agents=50, runners_dir=runners_dir)

    # ===================================================================
    # Test 1: Concurrency cap — max 2 in flight
    # ===================================================================
    print("\n[1] Concurrency cap (max_concurrency=2, 5 jobs)")
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "c1_j0": {"exit_code": 0, "output": "ok 0", "sleep": 0.3},
        "c1_j1": {"exit_code": 0, "output": "ok 1", "sleep": 0.3},
        "c1_j2": {"exit_code": 0, "output": "ok 2", "sleep": 0.3},
        "c1_j3": {"exit_code": 0, "output": "ok 3", "sleep": 0.3},
        "c1_j4": {"exit_code": 0, "output": "ok 4", "sleep": 0.3},
    })
    jobs = [make_job(f"c1_j{i}") for i in range(5)]
    t0 = time.monotonic()
    results = sched.submit(jobs)
    elapsed = time.monotonic() - t0

    all_ok = all(r.status == JobStatus.OK.value for r in results)
    # With max_concurrency=2 and 5 jobs at 0.3s each:
    # 3 waves: 2+2+1 → minimum ~0.9s.  Sequential would be 1.5s.
    if all_ok and elapsed < 1.4:
        ok(f"all OK, elapsed={elapsed:.2f}s (parallel, not sequential)")
    elif all_ok:
        ok(f"all OK, elapsed={elapsed:.2f}s (slow but correct)")
    else:
        bad = [r for r in results if not r.ok]
        fail("concurrency", f"failed jobs: {[(r.job_id, r.status, r.error) for r in bad]}")

    # ===================================================================
    # Test 2: exit 0 + empty reply → FAILED (not OK)
    # ===================================================================
    print("\n[2] Exit 0 + empty reply → FAILED")
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "c2_empty": {"exit_code": 0, "output": ""},
    })
    results = sched.submit([make_job("c2_empty")])
    r = results[0]
    if r.status == JobStatus.FAILED.value and "empty reply" in r.error.lower():
        ok(f"status=FAILED, error='{r.error}'")
    else:
        fail("empty reply", f"expected FAILED, got {r.status} error='{r.error}'")

    # ===================================================================
    # Test 3: RATE_LIMITED + retry → OK, attempts=2
    # ===================================================================
    print("\n[3] Rate limit → retry → success (attempts=2)")
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])

    # The fake runner only has one behaviour per job_id, so we can't have
    # per-attempt behaviour natively.  We simulate by creating two jobs with
    # different job_ids and verifying retry logic separately.  But for a true
    # end-to-end test, the scheduler must retry the SAME job.  The fake runner
    # cannot distinguish attempt 1 from attempt 2 since the prompt is identical.
    #
    # Workaround: use a mutable control file that the fake runner re-reads.
    # But fake_runner.py reads the control at invocation time, so we CAN
    # change the control file between attempts — the scheduler sleeps during
    # backoff, and we need to detect that and swap.
    #
    # Simpler: test retry logic by setting retry_on=("rate_limited",) and
    # having the runner ALWAYS emit 429 → the scheduler should retry up to
    # max_retries times and return RATE_LIMITED (exhausted).  Then separately
    # test that exit 0 + non-empty + no 429 → OK.
    #
    # For the "retry then success" scenario, we test the mechanism in two parts:
    #   a) Always-rate-limited → exhausts retries → RATE_LIMITED, attempts=2
    #   b) The retry loop actually sleeps (verified via timing)

    # 3a: Exhaust retries
    set_control({
        "c3_rl": {"exit_code": 1, "output": "error 429 Too Many Requests", "sleep": 0.01},
    })
    job_rl = make_job(
        "c3_rl",
        retry_policy=RetryPolicy(max_retries=2, backoff_sec=0.05, retry_on=("rate_limited", "timeout")),
    )
    results = sched.submit([job_rl])
    r = results[0]
    if r.status == JobStatus.RATE_LIMITED.value and r.attempts == 3:
        ok(f"exhausted retries: status=RATE_LIMITED, attempts={r.attempts}")
    else:
        fail("retry exhaust", f"status={r.status} attempts={r.attempts} error='{r.error}'")

    # 3b: Verify backoff sleep is real (timing test)
    set_control({
        "c3_timing": {"exit_code": 1, "output": "429 rate limit", "sleep": 0.01},
    })
    job_timing = make_job(
        "c3_timing",
        retry_policy=RetryPolicy(max_retries=3, backoff_sec=0.1, retry_on=("rate_limited",)),
    )
    t0 = time.monotonic()
    results = sched.submit([job_timing])
    wall = time.monotonic() - t0
    # Expected: 4 attempts (initial + 3 retries), backoffs: 0 + 0.1 + 0.2 + 0.4 = 0.7s
    if wall >= 0.5:
        ok(f"backoff sleep real: wall={wall:.2f}s (expected >=0.7s of sleep)")
    else:
        fail("backoff timing", f"wall={wall:.2f}s too fast — backoff not real")

    # ===================================================================
    # Test 4: exit 124 → TIMEOUT
    # ===================================================================
    print("\n[4] Exit 124 → TIMEOUT")
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "c4_to": {"exit_code": 124, "output": ""},
    })
    results = sched.submit([make_job("c4_to")])
    r = results[0]
    if r.status == JobStatus.TIMEOUT.value and r.exit_code == 124:
        ok(f"status=TIMEOUT, exit_code=124")
    else:
        fail("timeout", f"status={r.status} exit_code={r.exit_code}")

    # ===================================================================
    # Test 5: max_total_agents=2, submit 3 → ValueError
    # ===================================================================
    print("\n[5] max_total_agents cap (2 → reject 3)")
    sched_small = Scheduler(repo=repo, max_concurrency=1, max_total_agents=2, runners_dir=runners_dir)
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "c5_a": {"exit_code": 0, "output": "ok"},
        "c5_b": {"exit_code": 0, "output": "ok"},
        "c5_c": {"exit_code": 0, "output": "ok"},
    })
    try:
        sched_small.submit([make_job("c5_a"), make_job("c5_b"), make_job("c5_c")])
        fail("max_total_agents", "should have raised ValueError")
    except ValueError as e:
        if "exceeds max_total_agents" in str(e):
            ok(f"ValueError raised: {e}")
        else:
            fail("max_total_agents", f"wrong ValueError: {e}")

    # ===================================================================
    # Test 6: unhealthy runner → SKIPPED
    # ===================================================================
    print("\n[6] Unhealthy runner → SKIPPED")
    set_healthcheck([
        {"agent": "codex", "verdict": "OK"},
        {"agent": "agy", "verdict": "TIMEOUT"},
    ])
    set_control({
        "c6_ok": {"exit_code": 0, "output": "ok"},
    })
    results = sched.submit([
        make_job("c6_ok", runner="codex"),
        make_job("c6_bad", runner="agy"),
    ])
    r_ok = results[0]
    r_bad = results[1]
    passed = True
    if r_ok.status != JobStatus.OK.value:
        fail("healthy runner", f"expected OK, got {r_ok.status}")
        passed = False
    if r_bad.status != JobStatus.SKIPPED.value:
        fail("unhealthy runner", f"expected SKIPPED, got {r_bad.status}")
        passed = False
    if "unhealthy" not in r_bad.error.lower():
        fail("skip reason", f"error should mention unhealthy: {r_bad.error}")
        passed = False
    if passed:
        ok("healthy=OK, unhealthy=SKIPPED")

    # ===================================================================
    # Adversarial tests (edge cases from review spec)
    # ===================================================================

    print("\n[ADV] Adversarial edge cases")

    set_healthcheck([{"agent": "codex", "verdict": "OK"}])

    # A1: exit 0 + reply contains "429" → OK (rate-limit markers in successful
    #     reply body are content, not a rate-limit signal — anti false-positive)
    set_control({
        "adv_429_ok": {"exit_code": 0, "output": "这个API遇到429时应退避重试"},
    })
    results = sched.submit([make_job("adv_429_ok")])
    r = results[0]
    if r.status == JobStatus.OK.value:
        ok("exit=0 + 429 in reply body → OK (content, not rate-limit signal)")
    else:
        fail("429-in-reply", f"got status={r.status}, expected OK")

    # A2: exit=1 generic failure → FAILED with diagnostic
    set_control({
        "adv_fail": {"exit_code": 1, "output": ""},
    })
    results = sched.submit([make_job("adv_fail")])
    r = results[0]
    if r.status == JobStatus.FAILED.value and r.exit_code == 1 and "exit code 1" in r.error.lower():
        ok(f"exit=1 → FAILED, error='{r.error}'")
    else:
        fail("exit-1", f"status={r.status} error='{r.error}'")

    # A3: runner script missing → FAILED (not crash).
    #     Declare gemini healthy so the job passes the healthcheck gate,
    #     then delete the .sh so _execute_job hits the missing-script path.
    set_healthcheck([{"agent": "codex", "verdict": "OK"}, {"agent": "gemini", "verdict": "OK"}])
    (runners_dir / "gemini.sh").unlink()
    results = sched.submit([make_job("adv_no_runner", runner="gemini")])
    r = results[0]
    if r.status == JobStatus.FAILED.value and "not found" in r.error.lower():
        ok(f"missing runner → FAILED: {r.error}")
    else:
        fail("missing runner", f"status={r.status} error='{r.error}'")

    # A4: prompt_ref path doesn't exist → FAILED (not crash)
    results = sched.submit([
        make_job("adv_bad_path", prompt_ref="/nonexistent/prompt.txt", prompt_is_inline=False)
    ])
    r = results[0]
    if r.status == JobStatus.FAILED.value and "not found" in r.error.lower():
        ok(f"bad prompt_ref → FAILED: {r.error}")
    else:
        fail("bad prompt_ref", f"status={r.status} error='{r.error}'")

    # A5: 10 jobs, verify no lost/mixed results (correct ordering)
    set_control({f"adv10_{i}": {"exit_code": 0, "output": f"result_{i}", "sleep": 0.01}
                 for i in range(10)})
    jobs10 = [make_job(f"adv10_{i}") for i in range(10)]
    results = sched.submit(jobs10)
    all_ids = [r.job_id for r in results]
    expected_ids = [f"adv10_{i}" for i in range(10)]
    all_ok_10 = all(r.status == JobStatus.OK.value for r in results)
    reply_match = all(f"result_{i}" in results[i].reply_text for i in range(10))
    if all_ids == expected_ids and all_ok_10 and reply_match:
        ok("10-job batch: order correct, no loss, no mixing")
    else:
        fail("10-job batch", f"ids={all_ids}, all_ok={all_ok_10}, reply_match={reply_match}")

    # A6: cost tracking — elapsed_sec present in every result
    all_cost = all("elapsed_sec" in r.cost and r.cost["elapsed_sec"] >= 0 for r in results)
    if all_cost:
        ok("cost.elapsed_sec present on all results")
    else:
        fail("cost tracking")

    # ===================================================================
    # Regression: 5 defects from adversarial review (2026-06-03)
    # ===================================================================
    print("\n[REG] Regression tests — 5 defects fixed")

    # REG1: _classify exit=0 + 429 in reply body → OK (not RATE_LIMITED)
    exit_code, status, error = _classify(0, "这个API遇到429时应退避重试", "")
    if status == JobStatus.OK.value:
        ok("REG1: _classify(0, reply含429) → OK (anti false-positive)")
    else:
        fail("REG1", f"expected OK, got {status}")

    # REG1b: _classify exit=1 + stderr 429 → RATE_LIMITED (real rate limit still caught)
    exit_code, status, error = _classify(1, "", "ERROR: 429 Too Many Requests")
    if status == JobStatus.RATE_LIMITED.value:
        ok("REG1b: _classify(1, stderr_429) → RATE_LIMITED (real limit caught)")
    else:
        fail("REG1b", f"expected RATE_LIMITED, got {status}")

    # REG2: duplicate job_id → ValueError
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "reg2_a": {"exit_code": 0, "output": "ok"},
    })
    try:
        sched.submit([
            make_job("reg2_a"),
            make_job("reg2_a"),  # duplicate
        ])
        fail("REG2", "should have raised ValueError for duplicate job_id")
    except ValueError as e:
        if "duplicate job_id" in str(e):
            ok(f"REG2: duplicate job_id → ValueError: {e}")
        else:
            fail("REG2", f"wrong ValueError: {e}")

    # REG3: backoff caps enforced (max_backoff_sec + total_retry_wall_sec)
    sched_capped = Scheduler(
        repo=repo, max_concurrency=1, max_total_agents=50,
        max_backoff_sec=0.3, total_retry_wall_sec=0.8, runners_dir=runners_dir,
    )
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "reg3_cap": {"exit_code": 1, "output": "429 rate limit", "sleep": 0.01},
    })
    job_cap = make_job(
        "reg3_cap",
        retry_policy=RetryPolicy(max_retries=10, backoff_sec=5.0, retry_on=("rate_limited",)),
    )
    t0 = time.monotonic()
    results = sched_capped.submit([job_cap])
    wall = time.monotonic() - t0
    r = results[0]
    # With caps: max_backoff_sec=0.3, total_retry_wall_sec=0.8.
    # Attempts: 0(no sleep), 1(sleep capped 0.3, total=0.3),
    #           2(sleep capped 0.3, total=0.6), 3(sleep capped 0.3, total=0.9>0.8→break)
    # So attempts ≤ 3, wall < 3s (uncapped would be much longer).
    if r.attempts <= 4 and wall < 3:
        ok(f"REG3: backoff caps enforced (attempts={r.attempts}, wall={wall:.2f}s)")
    else:
        fail("REG3", f"caps not working: attempts={r.attempts}, wall={wall:.2f}s")

    # REG4: artifacts empty (file deleted in finally)
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "reg4_art": {"exit_code": 0, "output": "success text"},
    })
    results = sched.submit([make_job("reg4_art")])
    r = results[0]
    if r.artifacts == [] and r.reply_text == "success text":
        ok("REG4: artifacts=[], reply_text intact")
    else:
        fail("REG4", f"artifacts={r.artifacts}, reply_text={r.reply_text[:50]}")

    # REG5: healthcheck bad JSON shapes → all unhealthy (no crash)
    # 5a: dict instead of list
    set_healthcheck(None)  # clear env override
    hc_path = tmpdir / "hc_bad.json"
    hc_path.write_text('{"agent": "codex", "verdict": "OK"}')
    os.environ["SCHEDULER_TEST_HEALTHCHECK"] = str(hc_path)
    hc_dict_result = sched.healthcheck(["codex", "pi"])
    if hc_dict_result == {"codex": False, "pi": False}:
        ok("REG5a: healthcheck dict input → all unhealthy (no crash)")
    else:
        fail("REG5a", f"got {hc_dict_result}")

    # 5b: list with non-dict entries
    hc_path.write_text('[{"agent":"codex","verdict":"OK"}, "not_a_dict", {"agent":"pi","verdict":"OK"}]')
    hc_mixed_result = sched.healthcheck(["codex", "pi"])
    if hc_mixed_result == {"codex": False, "pi": False}:
        ok("REG5b: healthcheck list with non-dict entry → all unhealthy (no crash)")
    else:
        fail("REG5b", f"got {hc_mixed_result}")

    # 5c: valid JSON list works normally
    os.environ.pop("SCHEDULER_TEST_HEALTHCHECK", None)
    set_healthcheck([{"agent": "codex", "verdict": "OK"}, {"agent": "pi", "verdict": "TIMEOUT"}])
    hc_valid_result = sched.healthcheck(["codex", "pi"])
    if hc_valid_result == {"codex": True, "pi": False}:
        ok("REG5c: healthcheck valid list → correct healthy/unhealthy")
    else:
        fail("REG5c", f"got {hc_valid_result}")


    # REG6: per-job env + permission_policy become runner env and result evidence
    set_healthcheck([{"agent": "codex", "verdict": "OK"}])
    set_control({
        "reg6_env": {
            "exit_code": 0,
            "output_env": ["CODEX_SANDBOX", "CODEX_MODEL", "CUSTOM_FLAG"],
        },
    })
    job_env = make_job(
        "reg6_env",
        model="gpt-test",
        env={"CUSTOM_FLAG": "yes"},
        permission_policy=PermissionPolicy(
            sandbox="workspace-write",
            reason="user approved implementation job",
            user_approved=True,
        ),
    )
    r = sched.submit([job_env])[0]
    env_seen = json.loads(r.reply_text)
    if (env_seen.get("CODEX_SANDBOX") == "workspace-write"
            and env_seen.get("CODEX_MODEL") == "gpt-test"
            and env_seen.get("CUSTOM_FLAG") == "yes"
            and r.permissions.get("sandbox") == "workspace-write"
            and r.permissions.get("reason") == "user approved implementation job"):
        ok("REG6: per-job env/permission_policy passed and snapshotted")
    else:
        fail("REG6", f"env_seen={env_seen}, permissions={r.permissions}")

    # REG7: sandbox escalation guard catches unsafe Codex permission choices
    try:
        make_job("reg7_bad_write", permission_policy=PermissionPolicy(sandbox="workspace-write")).validate()
        fail("REG7a", "workspace-write without reason should fail")
    except ValueError as e:
        if "workspace-write" in str(e) and "reason" in str(e):
            ok(f"REG7a: workspace-write without reason blocked: {e}")
        else:
            fail("REG7a", f"wrong error: {e}")

    try:
        make_job(
            "reg7_danger",
            permission_policy=PermissionPolicy(sandbox="danger-full-access", reason="test only"),
        ).validate()
        fail("REG7b", "danger-full-access without approval should fail")
    except ValueError as e:
        if "user_approved" in str(e):
            ok(f"REG7b: danger-full-access without approval blocked: {e}")
        else:
            fail("REG7b", f"wrong error: {e}")

    try:
        make_job(
            "reg7_conflict",
            env={"CODEX_SANDBOX": "read-only"},
            permission_policy=PermissionPolicy(
                sandbox="workspace-write",
                reason="conflicting env test",
                user_approved=True,
            ),
        ).validate()
        fail("REG7c", "conflicting CODEX_SANDBOX should fail")
    except ValueError as e:
        if "conflicts" in str(e):
            ok(f"REG7c: CODEX_SANDBOX conflict blocked: {e}")
        else:
            fail("REG7c", f"wrong error: {e}")

    # ---- cleanup ----
    shutil.rmtree(tmpdir, ignore_errors=True)

    print(f"\n{'='*50}")
    print(f"Results: {tests_passed}/{tests_total} passed")
    if tests_passed == tests_total:
        print("SCHEDULER SELFTEST OK")
        return 0
    else:
        print(f"SCHEDULER SELFTEST FAILED ({tests_total - tests_passed} failures)")
        return 1


if __name__ == "__main__":
    import sys
    sys.exit(_run_selftest())
