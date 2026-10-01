# Discarded-source audit (`map_err(|_| ..)`)

## Goal
Classify every production `map_err(|_| ..)` that drops the lower error, repair the ones that lose diagnostics with
no record, and make `check-rust-style.mjs` reject any new untagged or wrongly tagged site (full scan, not diff-only).

## Facts (2026-09-30)
- A lint already existed (`DISCARDED_SOURCE`), but it only looked at added lines and accepted any Chinese comment.
- Full non-test count: 302 sites (212 with a comment, 90 without). The earlier "86" was a `grep -v test` undercount.
- Fix rule from `docs/design-docs/error-handling.md`: variant carries `#[source]`, `From` + `?`; owner's completion
  record extracts `error_class` / `io_error_kind`. No logs at Engine boundary mapping points; no facade widening.

## Phases
1. [x] Lint scaffolding: `--list-discarded`, tag format `丢弃来源[类别]：理由`, category list, full scan.
2. [ ] Read-only classification (4 areas) -> `class_*.json` in scratchpad.
3. [ ] Repair LOST sites (owner-side), tag all remaining sites with their category.
4. [ ] Lint tests (positive/negative), docs update (`error-handling.md` scope + tag format).
5. [ ] Verify: node tests, check-rust-style, check-engine-repository, cargo check (workspace + uc-infra lan-compat), nextest of touched crates.
6. [ ] Report. Do not push before asking the user (PR132 already has 362 files).

## Out of scope
`.ok()` and `let _ =` discards (list as follow-up only).
