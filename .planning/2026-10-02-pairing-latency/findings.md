# Findings

Field evidence (Android joiner + Windows sponsor, 2026-10-02, bundles kept under the thread evidence folder, not in git):
- Joiner total 28.713 s; round 1 reply wait 13.761 s = sponsor `protocol_lock` wait 13.138 s behind periodic maintenance round 733 (group updates to 3 unreachable members, 3 x 4.5 s serial).
- Sponsor day stats: 702 maintenance rounds, 74 >= 5 s, 39 >= 10 s; `maintenance_synchronization` steps >= 3 s: 128 (max 429 s), group-update steps >= 3 s: 24. So any long step holds the permit, not only group updates.
- Joiner handover 5.31 s: quiesce drain ~1.0 s (do not shorten), `joiner_activate` 1.275 s (two DB opens with migrations), first complete_ack connect `locally_rejected` because `ApplicationRuntime::start` started the maintenance runtime before `activate_session`.

Design decisions:
- Permit stays owned by the same owner; revocation signal is raised by `execute_exclusively` and observed by the maintenance use case, which drops the whole worker run (work is persisted; crash-equivalent) and reports Deferred.
- Background start moved out of `ApplicationRuntime::start` into `begin_background_work`, called by `SessionSupervisor::install_active_session` after network activation.
