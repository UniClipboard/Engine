#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const suiteIndex = process.argv.indexOf("--suite");
const suite = suiteIndex >= 0 ? process.argv[suiteIndex + 1] : "smoke";
if (!new Set(["smoke", "admission_repository", "group_update_delivery"]).has(suite)) {
  throw new Error(`unknown performance suite: ${suite}`);
}

const root = resolve(import.meta.dirname, "../..");
const startedAt = new Date().toISOString();
const gitSha = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: root,
  encoding: "utf8",
}).trim();
const dirty = execFileSync("git", ["status", "--porcelain"], {
  cwd: root,
  encoding: "utf8",
}).trim().length > 0;
const rustc = execFileSync("rustc", ["--version"], {
  cwd: root,
  encoding: "utf8",
}).trim();
const runId = startedAt.replaceAll(":", "-");
const outputDirectory = resolve(root, "target/performance", runId);
mkdirSync(outputDirectory, { recursive: true });
writeFileSync(
  resolve(outputDirectory, "manifest.json"),
  `${JSON.stringify(
    {
      format_version: 1,
      suite,
      git_sha: gitSha,
      dirty,
      started_at: startedAt,
      platform: process.platform,
      architecture: process.arch,
      rustc,
      fixture: "synthetic-engine-storage-v2",
    },
    null,
    2,
  )}\n`,
);

const environment = { ...process.env };
if (suite === "smoke") {
  environment.UNICLIPBOARD_BENCH_SMOKE = "1";
}
const benchmarkFilter =
  suite === "smoke"
    ? undefined
    : suite === "admission_repository"
      ? "admission_repository"
      : "group_update_delivery";
const cargoArguments = [
  "bench",
  "-p",
  "uc-infra",
  "--locked",
  "--features",
  "test-util",
  "--bench",
  "admission_repository",
];
if (benchmarkFilter) {
  cargoArguments.push("--", benchmarkFilter);
}
execFileSync(
  "cargo",
  cargoArguments,
  { cwd: root, env: environment, stdio: "inherit" },
);
writeFileSync(
  resolve(outputDirectory, "result.json"),
  `${JSON.stringify({ format_version: 1, status: "passed" }, null, 2)}\n`,
);
process.stdout.write(`${outputDirectory}\n`);
