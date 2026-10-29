/**
 * Generate src/exocortex/macros.json from the Exocortex TUI's macro catalog
 * (tui/src/macros.ts), PAR-208:
 *
 *   /opt/bun/bin/bun scripts/sync-exocortex-macros.ts <exocortex-dir> [out-file]
 *
 * The catalog is evaluated with a sentinel macro environment, so daemon paths
 * stay `{{repoRoot}}`-style placeholders that the app fills from
 * `tools_available.macroEnvironment` at runtime (src/exocortex/macros.rs), and
 * `/tool uninstall` is emitted once as a `{{toolDir}}` template. `win32` holds
 * the expansions a Windows daemon's path separators change.
 */
import { writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const [exoDir, outArg] = process.argv.slice(2);
if (!exoDir) {
  console.error("usage: bun scripts/sync-exocortex-macros.ts <exocortex-dir> [out-file]");
  process.exit(2);
}
const outFile = outArg ?? join(dirname(import.meta.dir), "src/exocortex/macros.json");

interface Arg { name: string; desc: string; exact?: boolean }
const mod = await import(join(resolve(exoDir), "tui/src/macros.ts")) as {
  BUILT_IN_MACRO_LIST: Arg[];
  getMacroMap(env?: unknown): Record<string, string>;
  getMacroArgs(base?: string, env?: unknown): Record<string, Arg[]>;
};

const env = (pathStyle: "posix" | "win32") => ({
  repoRoot: "{{repoRoot}}",
  storageDir: "{{storageDir}}",
  externalToolsDir: "{{externalToolsDir}}",
  externalToolsTrashDir: "{{externalToolsTrashDir}}",
  pathStyle,
  installedToolDirs: ["{{toolDir}}"],
});

const item = ({ name, desc, exact }: Arg) => ({ name, desc, ...(exact ? { exact: true } : {}) });
const sorted = <T>(record: Record<string, T>) => Object.fromEntries(Object.entries(record).sort(([a], [b]) => a.localeCompare(b)));

const expansions = mod.getMacroMap(env("posix"));
const win32 = Object.fromEntries(Object.entries(mod.getMacroMap(env("win32"))).filter(([key, text]) => expansions[key] !== text));

let rev = "unknown";
const git = Bun.spawnSync(["git", "-C", exoDir, "rev-parse", "--short", "HEAD"]);
if (git.exitCode === 0) rev = git.stdout.toString().trim();

const catalog = {
  generated: `scripts/sync-exocortex-macros.ts from Exocortex tui/src/macros.ts (${rev}); do not edit`,
  items: mod.BUILT_IN_MACRO_LIST.map(item),
  args: sorted(Object.fromEntries(Object.entries(mod.getMacroArgs(undefined, env("posix"))).map(([key, list]) => [key, list.map(item)]))),
  expansions: sorted(expansions),
  win32: sorted(win32),
};

writeFileSync(outFile, `${JSON.stringify(catalog, null, 2)}\n`);
console.log(`sync-exocortex-macros: ${catalog.items.length} macros, ${Object.keys(expansions).length} expansions (${Object.keys(win32).length} win32) -> ${outFile}`);
