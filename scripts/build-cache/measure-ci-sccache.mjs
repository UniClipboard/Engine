#!/usr/bin/env node
// 在同一 runner、同一提交与工具链下对比编译缓存的冷写入与暖命中：每轮先删除空 target，
// 清零 sccache 统计后执行同一构建命令，记录墙钟时间与 sccache 统计。
// 缓存只覆盖可缓存的 rustc 编译；链接、build script 运行、proc-macro 等不可缓存部分与测试执行不在收益内。
//
//   node scripts/build-cache/measure-ci-sccache.mjs --out <目录> [--require-r2] -- <构建命令...>
//
// 输出 <目录>/{cold,warm}.json（sccache 原始统计）与 summary.json、summary.md；设置了
// GITHUB_STEP_SUMMARY 时追加同一份 Markdown。构建失败时立即以原退出码结束，不跳过、不重试。
import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";

const PHASES = ["cold", "warm"];

function fail(message) {
  console.error(`measure-ci-sccache: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const split = argv.indexOf("--");
  if (split < 0 || split === argv.length - 1) fail("缺少 -- 之后的构建命令");
  const options = argv.slice(0, split);
  const command = argv.slice(split + 1);
  let out;
  let requireR2 = false;
  for (let i = 0; i < options.length; i += 1) {
    if (options[i] === "--out") out = options[++i];
    else if (options[i] === "--require-r2") requireR2 = true;
    else fail(`未知参数：${options[i]}`);
  }
  if (!out) fail("缺少 --out");
  return { out: resolve(out), requireR2, command };
}

function sccache(...args) {
  const bin = process.env.SCCACHE_PATH || "sccache";
  const result = spawnSync(bin, args, { encoding: "utf8" });
  if (result.status !== 0) fail(`sccache ${args.join(" ")} 失败`);
  return result.stdout;
}

// 只删除仓库内的 target，防止误删外部共享构建目录。
function emptyTargetDir() {
  const repo = process.cwd();
  const target = resolve(repo, process.env.CARGO_TARGET_DIR || "target");
  const rel = relative(repo, target);
  if (!rel || rel.startsWith("..") || isAbsolute(rel)) fail("CARGO_TARGET_DIR 必须位于仓库目录内");
  rmSync(target, { recursive: true, force: true });
}

const sum = (perLanguage) => Object.values(perLanguage?.counts ?? {}).reduce((a, b) => a + b, 0);
const seconds = (d) => (d ? d.secs + d.nanos / 1e9 : 0);

function summarize(info, wallSeconds) {
  const s = info.stats;
  return {
    wall_seconds: Number(wallSeconds.toFixed(1)),
    cache_location_kind: info.cache_location.split(/[,:]/)[0],
    compile_requests: s.compile_requests,
    hits: sum(s.cache_hits),
    misses: sum(s.cache_misses),
    non_cacheable: s.non_cacheable_compilations + s.requests_not_cacheable,
    errors: sum(s.cache_errors) + s.cache_read_errors + s.cache_write_errors + s.cache_timeouts,
    cache_writes: s.cache_writes,
    cache_read_hit_seconds: Number(seconds(s.cache_read_hit_duration).toFixed(1)),
    cache_write_seconds: Number(seconds(s.cache_write_duration).toFixed(1)),
    not_cached_reasons: s.not_cached,
  };
}

function markdown(results, command) {
  const rows = PHASES.map((phase) => {
    const r = results[phase];
    return `| ${phase} | ${r.wall_seconds} | ${r.compile_requests} | ${r.hits} | ${r.misses} | ${r.non_cacheable} | ${r.errors} | ${r.cache_writes} | ${r.cache_location_kind} |`;
  });
  const reasons = PHASES.map(
    (phase) => `- ${phase} 不可缓存原因：\`${JSON.stringify(results[phase].not_cached_reasons)}\``,
  );
  return [
    "## 编译缓存冷/暖对照",
    "",
    `命令：\`${command.join(" ")}\`；每轮从空 target 开始。`,
    "",
    "| 轮次 | 墙钟秒 | 编译请求 | 命中 | 未命中 | 不可缓存 | 错误 | 写入 | 后端 |",
    "|---|---|---|---|---|---|---|---|---|",
    ...rows,
    "",
    ...reasons,
    "",
    "墙钟时间包含链接、build script 与不可缓存编译；测试执行不在本对照内。",
    "",
  ].join("\n");
}

const { out, requireR2, command } = parseArgs(process.argv.slice(2));
mkdirSync(out, { recursive: true });

const results = {};
for (const phase of PHASES) {
  emptyTargetDir();
  sccache("--zero-stats");
  const started = process.hrtime.bigint();
  const build = spawnSync(command[0], command.slice(1), { stdio: "inherit" });
  const wall = Number(process.hrtime.bigint() - started) / 1e9;
  if (build.status !== 0) {
    console.error(`measure-ci-sccache: ${phase} 构建失败`);
    process.exit(build.status ?? 1);
  }
  const info = JSON.parse(sccache("--show-stats", "--stats-format=json"));
  if (requireR2 && !info.cache_location.startsWith("s3")) fail(`${phase} 轮未使用 R2 后端`);
  writeFileSync(join(out, `${phase}.json`), `${JSON.stringify(info, null, 2)}\n`);
  results[phase] = summarize(info, wall);
}

const report = markdown(results, command);
writeFileSync(join(out, "summary.json"), `${JSON.stringify(results, null, 2)}\n`);
writeFileSync(join(out, "summary.md"), report);
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, report);
console.log(report);
