/**
 * LTO pi completion hook — mechanical (not self-report) completion signal for
 * `dispatch-goal --runner pi` running in a real tmux TUI.
 *
 * On `agent_end` (fired when a prompt finishes all its turns) this spawns
 * `lto agent-turn-completed`, which writes the agent.turn.completed event,
 * wakes any `lto events --wait` waiter, and (with --bell) rings the tmux bell
 * as a human-visible fallback if the wake path is ever missed.
 *
 * Loaded explicitly via `pi -e <this file>` so it works even under
 * `--no-extensions` (which keeps explicit -e paths). Best-effort: any failure
 * is swallowed so the hook never disrupts pi's own turn.
 *
 * Environment (set by the LTO dispatcher on the pi launch command):
 *   LTO_BIN     — path to the lto binary (default: "lto")
 *   LTO_REPO    — repo root that owns the .lto run (default: cwd)
 *   LTO_RUN_ID  — active run id (optional; lto routes by cwd if absent)
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { spawn } from "node:child_process";

export default function (pi: ExtensionAPI) {
  pi.on("agent_end", async (_event, _ctx) => {
    try {
      const bin = process.env.LTO_BIN || "lto";
      const repo = process.env.LTO_REPO || process.cwd();
      const args = [
        "--repo",
        repo,
        "agent-turn-completed",
        "--runner",
        "pi",
        "--source",
        "pi-agent-end-hook",
        "--bell",
      ];
      const runId = process.env.LTO_RUN_ID;
      if (runId) {
        args.push("--run-id", runId);
      }
      // Detached + unref so pi does not wait on this notifier.
      const child = spawn(bin, args, {
        stdio: "ignore",
        detached: true,
      });
      child.on("error", () => {});
      child.unref();
    } catch {
      // Never let a notifier failure disrupt pi's turn.
    }
  });
}
