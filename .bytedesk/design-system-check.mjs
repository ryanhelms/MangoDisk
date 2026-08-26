#!/usr/bin/env node
/** Verify a committed ByteDesk managed tree without requiring a plugin cache. */

import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { lstat, readFile, readdir } from "node:fs/promises";
import path from "node:path";

const METADATA = new Set([".design-system.json", ".managed-files.json", ".source-sha", "README.md"]);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function parseArgs(argv) {
  let dir = ".context/design-system";
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--dir") {
      dir = argv[index + 1];
      if (!dir || dir.startsWith("--")) throw new Error("--dir needs a value");
      index += 1;
    } else if (argv[index] === "--help" || argv[index] === "-h") {
      process.stdout.write("Usage: design-system-check [--dir .context/design-system]\n");
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${argv[index]}`);
    }
  }
  if (path.isAbsolute(dir) || dir.split(/[\\/]/).includes("..")) throw new Error(`unsafe --dir: ${dir}`);
  return path.resolve(process.cwd(), dir);
}

async function walk(root, prefix = "") {
  const files = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    const absolute = path.join(root, entry.name);
    if ((await lstat(absolute)).isSymbolicLink()) throw new Error(`symlink forbidden in managed tree: ${relative}`);
    if (entry.isDirectory()) files.push(...await walk(absolute, relative));
    else if (entry.isFile()) files.push(relative);
  }
  return files.sort();
}

async function readJson(file, label) {
  try {
    return JSON.parse(await readFile(file, "utf8"));
  } catch (error) {
    throw new Error(`${label} is unreadable: ${error.message}`);
  }
}

async function main() {
  const root = parseArgs(process.argv.slice(2));
  if (!existsSync(root)) throw new Error(`managed tree is missing: ${root}`);
  const managed = await readJson(path.join(root, ".managed-files.json"), "managed-file state");
  const state = await readJson(path.join(root, ".design-system.json"), "consumer state");
  if (managed.schemaVersion !== 1 || !Array.isArray(managed.files)) throw new Error("managed-file state has an unsupported schema");
  if (managed.app !== state.app || managed.sourceSha !== state.sourceSha) throw new Error("managed-file state and consumer state disagree");
  const sourceSha = (await readFile(path.join(root, ".source-sha"), "utf8")).trim();
  if (!/^[0-9a-f]{40}$/.test(sourceSha) || sourceSha !== state.sourceSha) throw new Error("managed source provenance is invalid");

  const expected = new Set();
  const failures = [];
  for (const file of managed.files) {
    if (!file.path || path.isAbsolute(file.path) || file.path.split(/[\\/]/).includes("..")) {
      failures.push(`unsafe managed path: ${file.path}`);
      continue;
    }
    expected.add(file.path.replaceAll("\\", "/"));
    const absolute = path.join(root, file.path);
    if (!existsSync(absolute)) failures.push(`missing: ${file.path}`);
    else {
      const bytes = await readFile(absolute);
      if (bytes.length !== file.size || sha256(bytes) !== file.sha256) failures.push(`corrupted: ${file.path}`);
    }
  }
  for (const relative of await walk(root)) {
    if (!expected.has(relative) && !METADATA.has(relative)) failures.push(`unexpected: ${relative}`);
  }
  if (failures.length > 0) {
    for (const failure of [...new Set(failures)]) process.stderr.write(`design-system drift: ${failure}\n`);
    return 1;
  }
  process.stdout.write(`design-system healthy: app=${state.app} source=${sourceSha}\n`);
  return 0;
}

try {
  process.exitCode = await main();
} catch (error) {
  process.stderr.write(`design-system-check: ${error.message}\n`);
  process.exitCode = 2;
}
