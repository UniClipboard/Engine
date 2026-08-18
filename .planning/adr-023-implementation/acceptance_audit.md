# ADR 023 acceptance audit

This file is working evidence and must not be committed. A requirement is `proved` only when the
named test exercises the stated boundary and has a recorded non-zero pass. `gap` means the
implementation may exist, but the acceptance statement is broader than the current direct proof.

## Requirement matrix

| # | ADR line | Requirement (short form) | Implementation and exact proof | Status |
|---:|---:|---|---|---|
| 1 | 1954 | Old credential survives removal and rejoin | `versioned_membership_history.rs`; `same_device_rejoins_as_a_new_instance_without_losing_old_credential` | proved |
| 2 | 1955 | Credential is the exact MLS signer and binds all admission material | `space_access_adapter.rs`, `admission_transaction.rs`; `prepared_join_signer_proves_facts_without_activating_a_group`, `sponsor_candidate_uses_only_members_active_in_verified_history` | proved |
| 3 | 1957 | Every event is authorized and verified at its exact parent | `versioned_membership_history.rs`; `parent_authorization_rejects_unactivated_removed_and_wrong_credential_authors` | proved |
| 4 | 1958 | Historical verification does not consult current projections | core-only verifier boundary; `removed_members_credential_still_verifies_its_past_events_for_a_new_device` | proved |
| 5 | 1959 | A-B-C, remove B, C adds D and D verifies B | V2 history plus real five-device flow; `removed_members_credential_still_verifies_its_past_events_for_a_new_device`, `five_devices_restore_full_sync_after_two_completed_removals_and_rejoins` | proved |
| 6 | 1960 | Removed B cannot extend the current branch | `versioned_membership_history.rs`; `parent_authorization_rejects_unactivated_removed_and_wrong_credential_authors` | proved |
| 7 | 1961 | Removed target can still return its historical removal decision | `versioned_membership_history.rs`; `removed_member_can_sign_its_decision_from_the_removals_exact_parent` | proved |
| 8 | 1962 | Candidate is not AddDevice; no add before durable Prepared | `admission_transaction.rs`; `durable_admission_becomes_complete_only_after_both_sides_save`, `pending_member_removal_before_commit_rejects_without_add` | proved |
| 9 | 1963 | No rollback after S2; permanent loss uses RemoveDevice | `admission_transaction.rs`; `pending_member_removal_after_commit_permanently_keeps_add_then_remove` | proved |
| 10 | 1964 | Pending-member removal has exactly three outcomes | `admission_transaction.rs`; `pending_member_removal_before_commit_rejects_without_add`, `pending_member_removal_after_commit_permanently_keeps_add_then_remove`, `pending_member_removal_races_commit_and_activation_without_partial_state` | proved |
| 11 | 1967 | attempt, join and ordinal persist once; invitation claim binds once | `admission_attempt_store.rs`; `durable_join_starts_once_and_survives_owner_restart`, `compacted_join_id_cannot_be_reused_by_another_attempt` | proved |
| 12 | 1969 | Cancel and S2 have one winner; late cancel does not remove | `admission_transaction.rs`; `durable_admission_cancel_and_commit_have_exactly_one_winner` | proved |
| 13 | 1972 | Sponsor pending removal races have the same three outcomes | same owner and race suite as #10; `pending_member_removal_races_commit_and_activation_without_partial_state` | proved |
| 14 | 1975 | Every business message and Ack survives loss, duplicate and reorder | `durable_admission_becomes_complete_only_after_both_sides_save` replays Candidate, Prepared, Commit, Applied, Complete and CompleteAck from persisted state; `out_of_order_durable_messages_leave_the_saved_stage_unchanged`, rejected-ack and exact-delivery-ack tests cover no-write rejection and Ack handling | proved |
| 15 | 1977 | Crashes before and after J0-S3/J3 converge | `durable_admission_becomes_complete_only_after_both_sides_save` reconstructs the owner from the durable repository after every admission stage and resumes the same attempt through J3; Cross-Space has per-phase restart coverage | proved |
| 16 | 1978 | Another qualified member can complete after sponsor loss with bound challenge | `third_member_completion_keeps_joiner_pending_until_helper_applies_its_update`, `completion_helper_creation_requires_the_exact_saved_challenge`, and `completion_helper_applies_only_its_bound_admission_update` | proved |
| 17 | 1981 | S2/J2 write failures expose no half-state | `admission_attempt_store.rs`; `attempt_and_membership_history_roll_back_together_on_write_failure` | proved |
| 18 | 1982 | Sponsor/joiner derive the same public commitment | `mls_group.rs`; `sponsor_and_joiner_derive_the_same_public_admission_commitment`, `admission_security_commitment_has_a_canonical_public_identity` | proved |
| 19 | 1983 | Exact staged output survives restart without regeneration | encrypted attempt persistence; `durable_join_reopens_the_exact_member_and_resume_material`, `durable_join_preparation_is_not_regenerated_after_restart` | proved |
| 20 | 1985 | S2-S3 branch is locked except exact candidate removal | transaction repository and owner; `pending_member_removal_races_commit_and_activation_without_partial_state` | proved |
| 21 | 1987 | History/security stay sealed until S3 | `admission_security_transition.rs`, transaction owner; `prepared_security_state_can_be_reopened_activated_or_discarded`, `durable_admission_becomes_complete_only_after_both_sides_save` | proved |
| 22 | 1989 | Recovery scan includes nonterminal, outbox and write-ahead work | repository scan and owner recovery; `restart_recovery_delivers_durable_outboxes_and_compacts_settled_terminal_attempts`, `sponsor_recovery_finishes_the_same_activation_after_completion_save_fails` | proved |
| 23 | 1990 | Candidate stays outside all ordinary behavior until each activation gate | unified scope in `convergence/mod.rs`; `v2_current_peer_scope_requires_a_permanent_activation_receipt`, `v2_joiner_scope_stays_closed_until_the_local_join_is_active` | proved |
| 24 | 1992 | Public JoinSpace exposes only Active, Pending, Rejected | Engine/binding result mapping; `join_space_contract_returns_a_tagged_active_result_with_both_identities`, `join_space_mapping_preserves_history_counts` | proved |
| 25 | 1993 | One profile owner owns join, cancel, recovery and reset boundaries | profile convergence assembly; `factory_reset_is_one_application_action`, `engine_start_finishes_an_interrupted_factory_reset_before_opening_a_new_session` | proved |
| 26 | 1996 | Public API surface contains only the specified operations | Engine operation/result definitions plus removal searches; `every_public_operation_has_a_stable_kind` | proved |
| 27 | 2000 | Snapshot adds only current_join and pending_inbound_member | contract DTOs/bindings; `device_trust_json_keeps_complete_snapshot_fields`, `pending_inbound_projection_shows_only_the_active_lineage_non_terminal_candidate` | proved |
| 28 | 2002 | Fresh and Cross-Space projections never expose staged target as current | projection owner; `pending_cross_space_join_keeps_the_source_space_scope`, `pending_inbound_projection_shows_only_the_active_lineage_non_terminal_candidate` | proved |
| 29 | 2004 | Cancel and RemoveMember remain distinct | separate owner entries; `durable_admission_cancel_and_commit_have_exactly_one_winner`, pending-removal three-test suite | proved |
| 30 | 2006 | Pending and repeated JoinSpace resume the same attempt | `admission_transaction.rs`; `durable_join_starts_once_and_survives_owner_restart`, `durable_join_start_reuses_the_saved_wire_identity_after_restart` | proved |
| 31 | 2007 | Local claim is final despite remote consume outcomes | consume result is persisted in the attempt owner; `invitation_consume_retry_is_no_write_and_terminal_compaction_waits_for_resolution` | proved |
| 32 | 2009 | One global admission slot with precise release rules | V3 repository transaction; `a_second_non_terminal_attempt_cannot_take_the_profile_slot`, `terminal_compaction_preserves_replay_result_and_reset_only_advances_the_floor` | proved |
| 33 | 2013 | AdmissionUnavailable leaves the same pending attempt untouched | transaction owner; `admission_unavailable_keeps_the_exact_pending_join` | proved |
| 34 | 2015 | Profile key protects long-lived facts; attempt key is deleted after reseal | `admission_key_manager.rs`, V3 store; `profile_key_survives_restart_and_attempt_wrapping_is_context_bound`, `terminal_compaction_preserves_replay_result_and_reset_only_advances_the_floor`, plaintext repository test | proved |
| 35 | 2019 | Target stays staged until one verified manifest promotion | `admission_space_transition.rs`; `durable_transition_promotes_database_blobs_manifest_and_target_access_together` | proved |
| 36 | 2021 | Cross-Space drains source and recovers forward from every phase | transition phases and simulated per-phase failures; `cross_space_activation_saves_complete_before_forward_only_recovery`, `durable_transition_promotes_database_blobs_manifest_and_target_access_together` | proved |
| 37 | 2023 | Resume challenge binds attempt and all three identities | `completion_recovery_challenge_binds_all_three_members_and_transport_identities`, `third_member_completion_keeps_joiner_pending_until_helper_applies_its_update`, and completion-recovery wire identity/version tests | proved |
| 38 | 2026 | Applied receipt is permanent history; Completion is local only | V2 history and local attempt terminal; `verified_history_persistence_round_trip_preserves_authority_and_receipts`, `activation_receipts_require_the_event_and_conflicts_fail_closed` | proved |
| 39 | 2028 | Receipt-before-event and successor-before-receipt are held and retried correctly | `activation_receipts_require_the_event_and_conflicts_fail_closed`, `paged_exchange_applies_a_receipt_before_its_members_later_event`, and encrypted `paged_history_resumes_after_restart_and_applies_only_when_complete` | proved |
| 40 | 2031 | Activation projection covers all five baselines and only subtracts permission | history baseline and current-scope suites; `verified_and_legacy_migrations_create_explicit_activation_baselines`, four V2 scope tests | proved |
| 41 | 2033 | Five legacy layouts use immutable real-version ciphertext fixtures and restart tests | behavioral importer tests pass, but no official immutable historical ciphertext fixture set is present on this machine | skipped: official historical fixture set unavailable |
| 42 | 2035 | LegacyAccepted is authenticated, local-only and preserves V1 | versioned history migration path; `verified_and_legacy_migrations_create_explicit_activation_baselines`, V1 evidence tests | proved |
| 43 | 2037 | Checkpoint identity ignores migrating-member proof differences | checkpoint core; `legacy_checkpoint_identity_is_independent_of_member_input_order`, `checkpoint_attestations_are_additive_and_do_not_change_checkpoint_identity` | proved |
| 44 | 2038 | Unknown event/decision/algorithm/protocol/storage versions map to UpgradeRequired | `unknown_event_decision_receipt_and_signature_versions_require_upgrade`, `unknown_profile_metadata_version_fails_closed`, existing outer-wire future-version tests | proved |
| 45 | 2039 | Five old migration layouts import read-only and never forge proof | private importer; `orphan_backup_is_preserved_for_manual_recovery`, `prepared_state_cleans_only_after_source_and_backup_verify`, `handshake_done_rewraps_verifies_and_finishes_target`, `swapped_state_verifies_main_before_cleanup`, `corrupt_or_inconsistent_state_preserves_all_artifacts` | proved |
| 46 | 2041 | History `/2`, pairing V10, storage V3; `/1` only outbound probe | wire/storage constants; `history_v2_wire_checks_version_before_decoding_the_body`, `durable_admission_business_messages_round_trip_on_v10`, `dial_identifies_a_sponsor_that_only_supports_the_legacy_pairing_protocol` | proved |
| 47 | 2044 | All frame/page/receipt/recovery-route limits run at boundary and over limit | pairing, history, and recovery wire frame tests cover 4 MiB; event and activation-receipt page tests cover 256/257; `durable_commit_accepts_256_recovery_routes_and_rejects_257` covers sender and untrusted decoder | proved |
| 48 | 2045 | V3 side-write, verify, publish pointer, guard old row | V3 store; `v2_state_is_side_written_verified_and_guarded_before_v3_activation`, `failed_v3_reopen_keeps_original_v2_ciphertext_and_no_active_pointer`, `reopen_finishes_v3_cleanup_without_changing_the_active_state` | proved |
| 49 | 2048 | Problem-baseline old binary cannot overwrite V3 guard | current guard transaction is tested, but no official problem-baseline executable is installed or configured on this machine | skipped: official baseline binary unavailable |
| 50 | 2050 | Production no longer writes V1 or falls back to V1 success | V3 repository and `/2` production route; removal search plus V3 migration tests | proved |
| 51 | 2051 | QueryMigrationProgress and public migration types are deleted everywhere | source/interface search is empty for the public symbols; `every_public_operation_has_a_stable_kind` | proved |
| 52 | 2053 | current_join and inbound projections follow ordinal/floor rules | V3 repository; `current_local_join_projection_uses_pending_then_latest_visible_ordinal`, pending inbound projection test | proved |
| 53 | 2056 | DeviceTrust revision is profile-global, atomic and monotonic with corruption checks | `profile_counter_corruption_fails_closed`, `stale_revision_and_counter_overflow_leave_profile_state_unchanged`, `profile_revision_remains_monotonic_during_cross_space_transition` | proved |
| 54 | 2058 | Outbox is a keyed set with exact evidence-based clearing and no ack-of-ack | attempt model/repository; `attempt_and_outbox_advance_atomically_and_survive_restart`, `delivery_ack_clears_only_the_exact_supported_outbox`, `terminal_updates_are_monotonic_and_preserve_delivery_records` | proved |
| 55 | 2062 | ResetSpace is no-op while busy; quiet reset preserves durable facts | profile owner; `reset_projection_is_atomic_and_requires_a_quiet_admission_repository`, `profile_reset_preparation_rejects_pending_join_without_hiding_it` | proved |
| 56 | 2065 | Factory reset is resumable, key-first, complete, and invalidates old runtime | profile lifecycle/reset owner; `engine_start_finishes_an_interrupted_factory_reset_before_opening_a_new_session`, real reset/session test in host contract | proved |
| 57 | 2069 | Lifecycle marker has only generation and phase in secure storage | `profile_lifecycle.rs`; `factory_reset_phase_survives_restart_and_generation_changes_only_after_clear` | proved |
| 58 | 2072 | Applied history is sole positive authority; gates only subtract | unified scope owner; V2 current-peer scope suite | proved |
| 59 | 2073 | Security recipients come only from active parent history | sponsor preparation; `sponsor_candidate_uses_only_members_active_in_verified_history` | proved |
| 60 | 2074 | DB/WAL/SHM/cache/index/log scans contain no sensitive plaintext | repository, search and real Engine scans; `admission_identity_security_state_and_messages_never_reach_sqlite_in_plaintext`, `render_fields_never_plaintext_on_disk`, `persisted_engine_text_image_preview_and_logs_do_not_leave_plaintext_on_disk` | proved |
| 61 | 2075 | Desktop, iOS, Android, HarmonyOS share one result contract | stable Engine plus UniFFI/NAPI mappings; 45 public-contract, 18 UniFFI and 7 OHOS tests recorded passed; physical-device items skipped | proved, 2 device checks skipped |
| 62 | 2076 | Multi-device acceptance reads receiver-saved content | `space_membership_auto_pairing_e2e.rs`; five-device and offline-delivery scenarios query the receiver after real send | proved |
| 63 | 2077 | Exact tests and repository checks pass | Final rerun: full workspace suite passed, including core history 30/30, application 815/815, infrastructure 769 passed with 4 pre-existing ignored, UniFFI public contract 22/22, and HarmonyOS boundary 4/4; locked metadata, workspace all-target check, formatting, architecture preflight, and diff checks passed | proved |

## Open proof gaps

The in-repository direct-proof gaps are closed. Items 41 and 49 remain explicit external-fixture skips;
they must not be reported as passed until the official historical ciphertext set and problem-baseline
binary are supplied and executed. Physical mobile-device checks remain the two skips already recorded
under item 61.

## Commit audit

- Stage 3's two adjacent implementation slices were combined into `08193a7` after all acceptance gaps
  were closed. The final tree remained exactly `c3f50f3e900de108098ba6b4f5ac2a57b21bcb86` before and
  after the local history rewrite.
- Stages 0 through 12 and the final audit now each have exactly one ordered commit. The pre-rewrite
  branch remains recoverable as `backup/adr-023-before-stage3-squash-20260817`.

## Final audit result

- Repository-owned implementation and automated acceptance are complete.
- Official historical ciphertext, the problem-baseline executable, and two physical-device checks remain skipped.
- The final audit commit must exclude this `.planning/` directory.
- The staged implementation history now satisfies the one-commit-per-stage requirement.
- Post-rewrite checks passed, and the full tested tree is unchanged. External sample/binary and physical-device skips remain exactly as recorded above.
