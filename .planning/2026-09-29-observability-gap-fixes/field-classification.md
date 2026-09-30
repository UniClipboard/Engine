# Field classification for the one-shot migration (draft, 2026-09-29)

Generated from the inventory of all 945 raw tracing macro sites (1445 field uses, 327 distinct names) on baseline
`1f525b83`. "Status" is the current runtime treatment: ALLOWED (written as text), OMITTED (reviewed, written as
`<omitted>`) or UNLISTED (test fixtures and a few names in neither list). "Proposed" is the catalog class the
codemod starts from; the compiler corrects Number vs Flag and mixed-type fields.

| Proposed class | Names | Uses | Behaviour change |
| --- | ---: | ---: | --- |
| IoKind | 1 | 288 | none |
| Identifier(random) | 11 | 250 | none (check one by one: blob_id, format_id) |
| Vocabulary(reviewed) | 49 | 489 | none (explicit adapter, same trust model as id()) |
| Literal | 6 | 25 | none |
| Number / Flag | 209 | 402 | none (numeric and bool values are written unchanged) |
| Redacted (drop the field at the call site) | 48 | 195 | removes a key whose value was always `<omitted>` |
| REVIEW-literal | 3 | 13 | to decide: `event`, `task`, `device_name` |

## Rows

| Name | Uses | Value forms | Status | Proposed |
| --- | ---: | --- | --- | --- |
| $index | 1 | ident | UNLISTED | Number/Flag |
| accepted | 6 | ident | OMITTED | Number/Flag |
| ack_kind | 3 | expr | ALLOWED | Vocabulary(reviewed) |
| active_member_count | 1 | num | OMITTED | Number/Flag |
| add_bytes_ms | 1 | num | OMITTED | Number/Flag |
| add_path_ms | 1 | num | OMITTED | Number/Flag |
| addr_count | 1 | num | OMITTED | Number/Flag |
| advanced | 1 | ident | OMITTED | Number/Flag |
| affected_device_count | 1 | num | OMITTED | Number/Flag |
| allow_overlay | 2 | ident | OMITTED | Number/Flag |
| allow_overlay_network_addrs | 2 | ident | OMITTED | Number/Flag |
| allow_relay_fallback | 1 | ident | OMITTED | Number/Flag |
| alpn | 1 | display | ALLOWED | Vocabulary(reviewed) |
| already_initialized | 1 | ident | OMITTED | Number/Flag |
| attempt | 12 | ident | OMITTED | Number/Flag |
| attempt_id | 2 | display | ALLOWED | Identifier(random) |
| attribution | 3 | debug | OMITTED | Redacted |
| backoff_ms | 1 | num | OMITTED | Number/Flag |
| baseline_ms | 2 | ident | OMITTED | Number/Flag |
| bind_port | 2 | debug, ident | OMITTED | Redacted |
| blob_hash | 2 | display | OMITTED | Redacted |
| blob_id | 11 | display | ALLOWED | Identifier(random) |
| blob_ref_count | 6 | ident, num | OMITTED | Number/Flag |
| breakdown | 1 | display | OMITTED | Redacted |
| budget_ms | 1 | num | OMITTED | Number/Flag |
| bytes | 11 | ident, num | OMITTED | Number/Flag |
| bytes_downloaded | 3 | ident | OMITTED | Number/Flag |
| bytes_len | 2 | num | UNLISTED | Number/Flag |
| bytes_reclaimed | 1 | ident | OMITTED | Number/Flag |
| bytes_reclaimed_mb | 1 | num | OMITTED | Number/Flag |
| bytes_written | 2 | ident | OMITTED | Number/Flag |
| cache_bytes | 1 | ident | OMITTED | Number/Flag |
| cache_entries_deleted | 1 | ident | OMITTED | Number/Flag |
| cache_files_removed | 1 | ident | OMITTED | Number/Flag |
| cache_hit | 1 | bool | OMITTED | Flag |
| cache_orphans_removed | 1 | ident | OMITTED | Number/Flag |
| cancelled | 1 | ident | OMITTED | Number/Flag |
| candidates | 2 | ident | OMITTED | Number/Flag |
| cap | 1 | const | UNLISTED | Number/Flag |
| category | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| cause | 1 | expr | OMITTED | Number/Flag |
| ciphertext_bytes | 1 | num | OMITTED | Number/Flag |
| circuit_tripped | 1 | ident | OMITTED | Number/Flag |
| cleanup_failed | 1 | ident | OMITTED | Number/Flag |
| code | 2 | ident | OMITTED | Number/Flag |
| code_hash | 6 | display | OMITTED | Redacted |
| completed | 1 | num | OMITTED | Number/Flag |
| completed_peer_count | 1 | ident | OMITTED | Number/Flag |
| compressed_size | 4 | ident, num | OMITTED | Number/Flag |
| congestion_controller | 3 | display | ALLOWED | Vocabulary(reviewed) |
| conn | 10 | display | OMITTED | Redacted |
| connect_ms | 1 | num | OMITTED | Number/Flag |
| consecutive_failures | 2 | ident | OMITTED | Number/Flag |
| content_hash | 5 | display | OMITTED | Redacted |
| context | 9 | ident | OMITTED | Number/Flag |
| converted_size | 1 | num | OMITTED | Number/Flag |
| cooldown_ms | 1 | num | OMITTED | Number/Flag |
| corrupted | 1 | num | OMITTED | Number/Flag |
| count | 7 | ident, int, num | OMITTED | Number/Flag |
| current | 4 | display | ALLOWED | Vocabulary(reviewed) |
| current_epoch | 1 | expr | OMITTED | Number/Flag |
| current_version | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| custom_relay_count | 2 | num | OMITTED | Number/Flag |
| data_name | 1 | debug | UNLISTED | Redacted |
| database_bytes | 1 | ident | OMITTED | Number/Flag |
| decrypted | 1 | ident | OMITTED | Number/Flag |
| decrypted_bytes | 1 | ident | OMITTED | Number/Flag |
| deferred_count | 1 | ident | OMITTED | Number/Flag |
| deferred_peer_count | 1 | ident | OMITTED | Number/Flag |
| deleted | 1 | ident | OMITTED | Number/Flag |
| demoted | 2 | ident | OMITTED | Number/Flag |
| dependency | 1 | ident | OMITTED | Number/Flag |
| derived | 1 | display | UNLISTED | Redacted |
| device_count | 2 | int, num | OMITTED | Number/Flag |
| device_id | 6 | display, ident | OMITTED | Redacted |
| device_name | 1 | lit | UNLISTED | REVIEW-literal |
| directory_available | 1 | num | OMITTED | Number/Flag |
| disable_relays | 2 | ident | OMITTED | Number/Flag |
| doc_table | 1 | display | ALLOWED | Vocabulary(reviewed) |
| download_ms | 1 | num | OMITTED | Number/Flag |
| dropped | 2 | debug, num | OMITTED | Redacted |
| dropped_count | 1 | num | OMITTED | Number/Flag |
| dropped_reps | 1 | num | OMITTED | Number/Flag |
| duplicate | 5 | ident | OMITTED | Number/Flag |
| elapsed_ms | 11 | num | OMITTED | Number/Flag |
| emitter | 1 | ident | OMITTED | Number/Flag |
| encrypted | 1 | ident | OMITTED | Number/Flag |
| encryption_initialized | 2 | bool | OMITTED | Flag |
| endpoint | 2 | display | OMITTED | Redacted |
| endpoint_id | 3 | display | OMITTED | Redacted |
| entries_deleted | 3 | ident | OMITTED | Number/Flag |
| entries_scanned | 2 | ident | OMITTED | Number/Flag |
| entry_id | 132 | display | ALLOWED | Identifier(random) |
| envelope_len | 1 | num | OMITTED | Number/Flag |
| error | 6 | dynerr | OMITTED | Redacted |
| error.type | 2 | lit | ALLOWED | Literal |
| error_category | 6 | debug, display, lit | ALLOWED | Vocabulary(reviewed) |
| error_code | 5 | const, ident | ALLOWED | Vocabulary(reviewed) |
| error_kind | 332 | debug, expr, ident, lit | ALLOWED | Vocabulary(reviewed) |
| error_stage | 1 | lit | ALLOWED | Literal |
| error_type | 5 | debug | ALLOWED | Vocabulary(reviewed) |
| errored | 6 | ident | OMITTED | Number/Flag |
| errors | 5 | ident, num | OMITTED | Number/Flag |
| evaluation | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| event | 10 | lit | OMITTED | REVIEW-literal |
| event_id | 10 | display | ALLOWED | Identifier(random) |
| event_kind | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| existing_entry_id | 12 | display | ALLOWED | Identifier(random) |
| existing_status | 1 | ident | OMITTED | Number/Flag |
| expected | 2 | display, ident | ALLOWED | Vocabulary(reviewed) |
| expected_schema_ver | 1 | const | OMITTED | Number/Flag |
| expired_files | 1 | num | OMITTED | Number/Flag |
| expires_at | 2 | display | OMITTED | Redacted |
| export_ms | 1 | num | OMITTED | Number/Flag |
| extracted_paths_count | 1 | ident | OMITTED | Number/Flag |
| failed | 1 | num | OMITTED | Number/Flag |
| failure | 5 | lit | ALLOWED | Literal |
| failure_reason | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| failure_stage | 8 | lit | ALLOWED | Literal |
| file_candidate_count | 1 | num | OMITTED | Number/Flag |
| file_paths_source | 1 | expr | OMITTED | Number/Flag |
| file_size | 3 | ident | OMITTED | Number/Flag |
| files_removed | 1 | ident | OMITTED | Number/Flag |
| filter_kind | 1 | num | ALLOWED | Vocabulary(reviewed) |
| fingerprint | 2 | display | OMITTED | Redacted |
| format_id | 4 | display | ALLOWED | Vocabulary(reviewed) |
| format_ids | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| found_schema_ver | 1 | ident | OMITTED | Number/Flag |
| freed_bytes | 1 | ident | OMITTED | Number/Flag |
| from | 2 | display | OMITTED | Redacted |
| from_device | 5 | display | OMITTED | Redacted |
| gc_interval_secs | 1 | num | OMITTED | Number/Flag |
| grace_ms | 1 | num | OMITTED | Number/Flag |
| grandfathered | 2 | ident | OMITTED | Number/Flag |
| group_epoch | 4 | expr, ident | OMITTED | Number/Flag |
| has_completed | 1 | ident | OMITTED | Number/Flag |
| has_current_invitation | 1 | num | OMITTED | Number/Flag |
| has_device_name | 1 | num | OMITTED | Number/Flag |
| has_kek | 2 | ident | OMITTED | Number/Flag |
| has_more | 2 | ident | OMITTED | Number/Flag |
| has_space | 1 | num | OMITTED | Number/Flag |
| hash | 15 | display | OMITTED | Redacted |
| hash_ms | 1 | ident | OMITTED | Number/Flag |
| healthy | 1 | ident | OMITTED | Number/Flag |
| id | 1 | debug | UNLISTED | Redacted |
| idx | 4 | ident | OMITTED | Number/Flag |
| indexed_paths | 1 | num | OMITTED | Number/Flag |
| ingest_failed | 2 | ident | OMITTED | Number/Flag |
| inline | 1 | ident | OMITTED | Number/Flag |
| intent | 2 | debug | ALLOWED | Vocabulary(reviewed) |
| io_error_code | 1 | expr | OMITTED | Number/Flag |
| io_error_kind | 288 | ident, iok | ALLOWED | IoKind |
| ip_addr_count | 1 | num | OMITTED | Number/Flag |
| ip_addrs | 1 | debug | OMITTED | Redacted |
| is_favorited | 3 | ident | OMITTED | Number/Flag |
| issue | 1 | expr | OMITTED | Number/Flag |
| item_type | 6 | debug | UNLISTED | Redacted |
| key_class | 2 | expr | ALLOWED | Vocabulary(reviewed) |
| kind | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| last_attempt_ms | 1 | ident | OMITTED | Number/Flag |
| latency_ms | 1 | ident | OMITTED | Number/Flag |
| limit | 1 | ident | OMITTED | Number/Flag |
| line_count | 2 | num | OMITTED | Number/Flag |
| line_index | 1 | ident | OMITTED | Number/Flag |
| local_path_count | 3 | ident, num | OMITTED | Number/Flag |
| logs_bytes | 1 | ident | OMITTED | Number/Flag |
| managed_total_mb | 2 | num | OMITTED | Number/Flag |
| max | 1 | const | UNLISTED | Number/Flag |
| max_attempts | 2 | const, ident | OMITTED | Number/Flag |
| mime | 7 | display, expr, ident | ALLOWED | Vocabulary(reviewed) |
| mimes | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| missed | 5 | ident | OMITTED | Number/Flag |
| missing | 1 | num | OMITTED | Number/Flag |
| missing_count | 1 | num | OMITTED | Number/Flag |
| missing_entries_deleted | 1 | ident | OMITTED | Number/Flag |
| mode | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| ms | 4 | ident | OMITTED | Number/Flag |
| new_effect_count | 1 | ident | OMITTED | Number/Flag |
| next_phase | 2 | debug | ALLOWED | Vocabulary(reviewed) |
| next_recheck_ms | 1 | num | OMITTED | Number/Flag |
| note | 1 | display | UNLISTED | Redacted |
| now_ms | 1 | ident | OMITTED | Number/Flag |
| offline | 6 | ident | OMITTED | Number/Flag |
| offset | 1 | ident | OMITTED | Number/Flag |
| on_disk_size | 4 | ident, num | OMITTED | Number/Flag |
| op | 3 | ident | ALLOWED | Vocabulary(reviewed) |
| operation | 18 | ident, lit | ALLOWED | Vocabulary(reviewed) |
| origin | 2 | debug | ALLOWED | Vocabulary(reviewed) |
| origin_guard_key | 3 | display | OMITTED | Redacted |
| original_mime | 2 | display | ALLOWED | Vocabulary(reviewed) |
| original_size | 1 | ident | OMITTED | Number/Flag |
| orphan_count | 2 | num | OMITTED | Number/Flag |
| orphaned | 1 | num | OMITTED | Number/Flag |
| orphans_removed | 1 | ident | OMITTED | Number/Flag |
| os_hash | 1 | display | OMITTED | Redacted |
| os_write_succeeded | 2 | ident | OMITTED | Number/Flag |
| outcome | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| packed_rep_count | 1 | num | OMITTED | Number/Flag |
| packed_rep_ids | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| page_count | 3 | ident, num | OMITTED | Number/Flag |
| page_number | 1 | num | OMITTED | Number/Flag |
| partial | 1 | ident | OMITTED | Number/Flag |
| partial_publication | 1 | bool | OMITTED | Flag |
| paste_rep_id | 2 | display | ALLOWED | Identifier(random) |
| path | 35 | const, display, ident, lit | OMITTED | Redacted |
| payload_state | 4 | debug | ALLOWED | Vocabulary(reviewed) |
| peer | 15 | display | OMITTED | Redacted |
| pending | 5 | ident | OMITTED | Number/Flag |
| pending_group_update_count | 2 | num | OMITTED | Number/Flag |
| phase | 2 | debug, ident | ALLOWED | Vocabulary(reviewed) |
| plain_rep_id | 1 | display | ALLOWED | Identifier(random) |
| plaintext_bytes | 1 | ident | OMITTED | Number/Flag |
| plaintext_len | 1 | num | OMITTED | Number/Flag |
| plaintext_size | 4 | ident, num | OMITTED | Number/Flag |
| plan | 1 | expr | OMITTED | Number/Flag |
| port | 1 | ident | OMITTED | Number/Flag |
| posting_table | 1 | display | ALLOWED | Vocabulary(reviewed) |
| preview_length_chars | 1 | const | OMITTED | Number/Flag |
| preview_rep_id | 2 | display | ALLOWED | Identifier(random) |
| previous_epoch | 1 | expr | OMITTED | Number/Flag |
| previous_phase | 2 | debug | ALLOWED | Vocabulary(reviewed) |
| profile_id | 4 | display, ident | OMITTED | Redacted |
| profile_ready | 1 | expr | OMITTED | Number/Flag |
| projections | 1 | num | OMITTED | Number/Flag |
| proof_len | 1 | num | OMITTED | Number/Flag |
| provider | 2 | display | OMITTED | Redacted |
| provider_failures | 2 | ident | OMITTED | Number/Flag |
| public_addr | 2 | debug, display | OMITTED | Redacted |
| publish_ms | 2 | ident | OMITTED | Number/Flag |
| published | 1 | num | OMITTED | Number/Flag |
| quota_mb | 2 | num | OMITTED | Number/Flag |
| reason | 31 | const, debug, display, ident, lit | ALLOWED | Vocabulary(reviewed) |
| received_page_count | 1 | ident | OMITTED | Number/Flag |
| reconcile_failed | 1 | ident | OMITTED | Number/Flag |
| reconstructed_hash | 1 | display | OMITTED | Redacted |
| recovered | 2 | ident | OMITTED | Number/Flag |
| recovered_after_failures | 1 | ident | OMITTED | Number/Flag |
| relay_url | 1 | ident | OMITTED | Number/Flag |
| relay_url_count | 1 | num | OMITTED | Number/Flag |
| relay_urls | 1 | debug | OMITTED | Redacted |
| remaining | 1 | num | OMITTED | Number/Flag |
| remaining_secs | 1 | num | OMITTED | Number/Flag |
| removed | 5 | ident | OMITTED | Number/Flag |
| rendezvous_override | 1 | num | OMITTED | Number/Flag |
| rep_count | 2 | num | OMITTED | Number/Flag |
| rep_formats | 2 | display | OMITTED | Redacted |
| rep_id | 5 | display | ALLOWED | Identifier(random) |
| reply_kind | 1 | expr | ALLOWED | Vocabulary(reviewed) |
| repr_count | 1 | num | OMITTED | Number/Flag |
| representation_count | 1 | num | OMITTED | Number/Flag |
| representation_id | 48 | display | ALLOWED | Identifier(random) |
| representation_index | 5 | ident | OMITTED | Number/Flag |
| representations | 3 | ident, num | OMITTED | Number/Flag |
| requested | 1 | display | UNLISTED | Redacted |
| response | 1 | debug | OMITTED | Redacted |
| result | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| retention_entries_deleted | 1 | ident | OMITTED | Number/Flag |
| retention_failed | 1 | ident | OMITTED | Number/Flag |
| retryable | 5 | bool, ident | OMITTED | Number/Flag |
| returned | 2 | num | OMITTED | Number/Flag |
| reused_existing | 3 | bool, ident | OMITTED | Number/Flag |
| rewritten | 1 | ident | OMITTED | Number/Flag |
| rewritten_rep_count | 1 | ident | OMITTED | Number/Flag |
| root_count | 4 | ident, num | OMITTED | Number/Flag |
| rules | 1 | debug | ALLOWED | Vocabulary(reviewed) |
| save_ref_ms | 2 | ident | OMITTED | Number/Flag |
| scope | 7 | display, ident | ALLOWED | Vocabulary(reviewed) |
| secret_count | 1 | num | OMITTED | Number/Flag |
| sender_is_bound | 1 | ident | OMITTED | Number/Flag |
| session_id | 7 | display | OMITTED | Redacted |
| settled_count | 1 | ident | OMITTED | Number/Flag |
| size | 1 | ident | UNLISTED | Number/Flag |
| size_bytes | 12 | ident, num | OMITTED | Number/Flag |
| size_cap_exceeded | 2 | ident | OMITTED | Number/Flag |
| skipped | 1 | ident | OMITTED | Number/Flag |
| snapshot_hash | 18 | display | OMITTED | Redacted |
| snapshot_rep_count | 1 | ident | OMITTED | Number/Flag |
| source | 6 | display, ident, lit | ALLOWED | Vocabulary(reviewed) |
| source_device_id | 1 | display | UNLISTED | Redacted |
| space_id | 3 | display | OMITTED | Redacted |
| sql | 2 | ident | OMITTED | Number/Flag |
| stable_failure_count | 1 | ident | OMITTED | Number/Flag |
| stage | 6 | debug, ident, lit | ALLOWED | Vocabulary(reviewed) |
| staged | 1 | ident | OMITTED | Number/Flag |
| staged_with_preview | 1 | ident | OMITTED | Number/Flag |
| status | 2 | display | OMITTED | Redacted |
| step | 6 | lit | ALLOWED | Literal |
| storage_generation | 1 | expr | OMITTED | Number/Flag |
| stored.attribution | 1 | debug | UNLISTED | Redacted |
| stored_hash | 1 | display | OMITTED | Redacted |
| stored_version | 1 | display | ALLOWED | Vocabulary(reviewed) |
| strategy | 3 | lit | ALLOWED | Literal |
| stripped_count | 1 | ident | OMITTED | Number/Flag |
| subnets | 1 | debug | OMITTED | Redacted |
| success | 2 | num | OMITTED | Number/Flag |
| summary | 1 | display | UNLISTED | Redacted |
| table | 4 | display | ALLOWED | Vocabulary(reviewed) |
| tag | 2 | display | OMITTED | Redacted |
| target_activated | 1 | ident | OMITTED | Number/Flag |
| target_epoch | 1 | ident | OMITTED | Number/Flag |
| task | 2 | ident, lit | OMITTED | REVIEW-literal |
| threshold | 5 | const, ident | OMITTED | Number/Flag |
| ticket_ms | 2 | ident | OMITTED | Number/Flag |
| to | 2 | display | OMITTED | Redacted |
| tolerance_ms | 1 | const | OMITTED | Number/Flag |
| total | 8 | ident | OMITTED | Number/Flag |
| total_bytes | 2 | ident | OMITTED | Number/Flag |
| total_ciphertext_bytes | 1 | ident | OMITTED | Number/Flag |
| total_entries | 1 | ident | OMITTED | Number/Flag |
| total_file_bytes | 1 | ident | OMITTED | Number/Flag |
| total_plaintext_bytes | 1 | ident | OMITTED | Number/Flag |
| total_size_bytes | 1 | num | OMITTED | Number/Flag |
| transfer_id | 25 | display, ident | ALLOWED | Identifier(random) |
| tried_providers | 3 | ident | OMITTED | Number/Flag |
| trigger | 1 | ident | OMITTED | Number/Flag |
| ttl_ms | 2 | num | OMITTED | Number/Flag |
| ttl_secs | 1 | ident | OMITTED | Number/Flag |
| uc_congestion_controller | 1 | display | ALLOWED | Vocabulary(reviewed) |
| uc_iroh_bind_port | 2 | display | OMITTED | Redacted |
| unsupported_member | 2 | ident | OMITTED | Number/Flag |
| upgrade_action | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| upgrade_phase | 1 | ident | ALLOWED | Vocabulary(reviewed) |
| variant | 2 | ident | ALLOWED | Vocabulary(reviewed) |
| vault_bytes | 1 | ident | OMITTED | Number/Flag |
| version | 1 | display | ALLOWED | Vocabulary(reviewed) |
| victim | 1 | display | UNLISTED | Redacted |
| visible_roots | 2 | ident | OMITTED | Number/Flag |
