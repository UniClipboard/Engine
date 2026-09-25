# Task Plan: Upgrade compatibility matrix (plan 051)

Plan doc: `docs/exec-plans/active/051-upgrade-compatibility-matrix.md`. Planning files live here.

## Decisions (user, 2026-09-25)
- Anchors: Desktop 1.0.0 published releases -> locked Engine rev (not Engine tags). 13 old revs + HEAD.
- Combinations: all ordered pairs i<j + one full chain.
- Dimensions: single-device upgrade, sequential two-device upgrade, mixed-version interop, downgrade.
- Cadence: PR smoke (A13->HEAD) + nightly/manual full matrix.

## Phases
- [x] Research + plan draft
- [x] Open questions answered (downgrade per recommendation; unpublished builds not delivered; content scope enough)
- [ ] S0..S7 (see plan doc)
