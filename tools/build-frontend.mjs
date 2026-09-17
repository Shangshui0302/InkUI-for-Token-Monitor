import { chmod, copyFile, mkdir, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const script = fileURLToPath(import.meta.url);
const root = resolve(dirname(script), "..");
const ui = resolve(root, "apps/ui");
const dist = resolve(ui, "dist");
const inputs = [script, resolve(ui, "package.json"), resolve(ui, "index.html"), resolve(ui, "styles.css"), resolve(ui, "src/main.tsx")];
const outputs = ["index.html", "styles.css", "app.js"].map((name) => resolve(dist, name));

async function exists(path) { try { await stat(path); return true; } catch { return false; } }
async function stale() {
  if (!(await Promise.all(outputs.map(exists))).every(Boolean)) return true;
  const newestInput = Math.max(...(await Promise.all(inputs.map(async (path) => (await stat(path)).mtimeMs))));
  const oldestOutput = Math.min(...(await Promise.all(outputs.map(async (path) => (await stat(path)).mtimeMs))));
  return newestInput > oldestOutput;
}
function run(command, args, options = {}) {
  return new Promise((ok, fail) => {
    const child = spawn(command, args, { cwd: root, stdio: "inherit", shell: process.platform === "win32", ...options });
    child.on("error", fail); child.on("exit", (code) => code === 0 ? ok() : fail(new Error(`${command} exited with ${code}`)));
  });
}

if (process.env.NEKODEX_FRONTEND_PREBUILT === "1") {
  if (!(await Promise.all(outputs.map(exists))).every(Boolean)) {
    throw new Error("NEKODEX_FRONTEND_PREBUILT requires index.html, styles.css, and app.js under apps/ui/dist.");
  }
  console.log("Using explicitly provided frontend dist.");
  process.exit(0);
}
if (!(await stale()) && process.env.FRONTEND_FORCE_BUILD !== "1") {
  console.log("Frontend dist is up to date; skipping build.");
  process.exit(0);
}
await mkdir(dist, { recursive: true });
// Nix outputs are read-only. Make previous copies writable before replacing
// them, while preserving the last successful build if the next build fails.
await Promise.all(outputs.map(async (path) => {
  if (await exists(path)) await chmod(path, 0o644);
}));
const localEsbuild = resolve(ui, "node_modules/.bin", process.platform === "win32" ? "esbuild.cmd" : "esbuild");
const nativeEsbuild = process.env.ESBUILD || (await exists(localEsbuild) ? localEsbuild : "esbuild");
if (await exists(resolve(ui, "node_modules/react/index.js")) && await exists(resolve(ui, "node_modules/react-dom/client.js"))) {
  await run(nativeEsbuild, ["src/main.tsx", "--bundle", "--minify", "--format=iife", "--target=chrome108", "--jsx=automatic", "--define:process.env.NODE_ENV=\"production\"", "--outfile=dist/app.js"], { cwd: ui });
  await copyFile(resolve(ui, "index.html"), resolve(dist, "index.html"));
  await copyFile(resolve(ui, "styles.css"), resolve(dist, "styles.css"));
} else {
  throw new Error("Frontend dependencies are missing. Install apps/ui/package-lock.json dependencies in the development environment before building; the script will not invoke Nix implicitly.");
}
