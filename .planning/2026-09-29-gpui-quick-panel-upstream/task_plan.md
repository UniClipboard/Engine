# GPUI quick panel upstream (Engine thread uni-t-0110, requester uni-t-0108)

Baseline: Engine HEAD c7a821b4 == the rev Desktop pins (Cargo.toml `uc-engine` rev c7a821b4d899d32b375a707d2551f382e088b306).
Worktree branch: hp/uni/t-0110-gpui-engine-desktop-t-0108. t-0106 (LAN-only) not touched: this work stays in search / settings event / uc-engine contract.

## Ownership decisions (verified against code)

| Item | Engine | Desktop daemon (downstream task) |
| --- | --- | --- |
| U1 content-lock grant | nothing new: UnlockSpace / RecoverSession / QueryEncryptionState / QueryProfileRecovery / ProfileRecoveryChanged / settings.security.auto_unlock_enabled already exist | in-memory grant, GET/POST routes, WS topic `content-lock`. The grant is a GUI authorisation, not an Engine invariant; visible = grant && session_ready computed at read time, so no atomic coupling with the session is needed |
| U2 settings push | `EngineEvent::SettingsChanged { sections }` after every successful settings write (UpdateSettings, SaveRelay, MutateCustomRelay, UpdateDebugMode, config import) | WS topic `settings`, map event -> topic, clients refetch GET /settings |
| U3 tag all/any | `tag_match` on SearchQuery / SearchEntriesInput | `tagMode` query param |
| U4 counts | `Operation::CountSearchEntries` (batch), same parse + same index path as search | route `/search/count` |
| U5 daily counts | `Operation::QueryEntryDailyCounts { boundaries_ms }` (caller supplies local-day boundaries, no tz/DST in Engine) | route `/search/daily-counts` |
| U6 classification | **Withdrawn by user decision; removed.** No fields, inference, payload change, backfill or rebuild. | none |
| U7 match spans | not done (index holds HMAC tokens, no plaintext positions) | client highlights |

## Phases
1. failing tests -> 2. U3 -> 3. U4 -> 4. U5 -> 5. U6 -> 6. U2 -> 7. docs/contract/report

## Update
U6 removed after implementation at the user's request. Remaining: U2, U3, U4, U5 (U1 downstream-only, U7 not done). Verification re-run after removal; no new unit tests added.
