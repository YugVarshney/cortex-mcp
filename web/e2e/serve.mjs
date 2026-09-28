// Starts one production-shaped Recall-MCP server: the Rust binary serving the
// built UI (web/dist) and the API from a throwaway database. Playwright's
// webServer waits on /healthz before running tests.
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");
const exe = join(repoRoot, "target", "debug", process.platform === "win32" ? "recall-cli.exe" : "recall-cli");
const webDir = join(repoRoot, "web", "dist");
const db = join(mkdtempSync(join(tmpdir(), "recall-e2e-")), "e2e.db");

const child = spawn(exe, ["serve", "--bind", "127.0.0.1:8799", "--db", db, "--web-dir", webDir], {
  stdio: ["ignore", "inherit", "inherit"],
});

const shutdown = () => {
  child.kill();
  process.exit(0);
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
child.on("exit", (code) => process.exit(code ?? 0));
