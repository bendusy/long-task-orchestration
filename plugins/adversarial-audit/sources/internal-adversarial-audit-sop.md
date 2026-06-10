# Internal SOP: Heterogeneous Adversarial Audit

> Source type: internal operational distillation  
> Origin: repeated multi-agent audit sessions (agent-skills repo, LTO Phase 5–6)  
> Status: experimental — claims unverified at scale

## Core rules

### 1. Heterogeneity is mandatory

The auditor squad must not share a model family with the artifact producer.  
Default squad: **codex + pi + agy** (three distinct runtimes/families).  
Same-family self-audit is invalid regardless of prompt strength.

### 2. Two merge rules — never conflate them

| Operation | Rule | Rationale |
|---|---|---|
| **Findings merge** | Union — every finding from every auditor enters the blocker register | Voting drops real blockers that only one auditor caught |
| **Direction decision** | 2/3 majority vote + host agent review | Avoids deadlock; minority dissent escalated to human |

### 3. Refute-first prompting

Every auditor's instruction is to **REFUTE** — assume the artifact is wrong and attempt to prove it.  
When uncertain, lean toward `refuted` rather than `pass`.  
Empty findings (rubber-stamp PASS) must trigger a re-run with stronger posture.

### 4. Evidence contract

Every finding must cite `path:line` or a verbatim command output.  
"LLM says there is a problem" does not qualify as evidence.  
Findings without evidence are auto-classified `low` and flagged `unsupported`.

### 5. Convergence criteria

The blocker register converges when **all** of:
- HIGH + CRITICAL count is monotonically non-increasing across rounds
- Two consecutive rounds produce zero new HIGH/CRITICAL blockers
- Every adopted/rejected item has a first-hand evidence record

### 6. Known anti-patterns

**ap1 — same-family self-audit**: codex reviewing codex output, or claude reviewing claude output.  
**ap2 — vote-to-merge findings**: majority vote silently drops valid blockers.  
**ap3 — historical-contrast hallucination**: showing auditor "before → after" causes it to re-report the `before` state as current. Strip historical context; describe current state only.  
**ap4 — rubber-stamp PASS**: zero findings ≠ quality. Treat as audit failure unless auditor provides explicit proof-of-read evidence.

## Playbook outline

```
1. Fan-out: dispatch codex-refuter + pi-refuter + agy-refuter in parallel on same artifact
2. Collect: gather findings JSON from each runner
3. Union-merge: concatenate all findings into blocker register (no deduplication at this stage)
4. Host triage: for each blocker, host reads cited path:line, classifies adopt/reject/needs-more-info
5. Convergence check: HIGH+CRITICAL count ≤ previous round; if two rounds clean → done
6. Iterate: if not converged, fix adopted blockers, re-run squad on updated artifact
```

## Direction decisions (when applicable)

When auditors disagree on a direction (not a finding), apply:
- 2/3 majority → tentative decision
- Host agent reviews minority argument
- Still unresolved → escalate to human
