# Progress

- 2026-08-12: Replaced the earlier broad version/channel compatibility rule.
  ADR-020 now distinguishes only peers below `1.1` and peers running this
  `1.1` release. The current reconciliation path serves `1.1` peers only.
  A `1.1` device preserves discovery, Space, and member data for an older
  peer, blocks content in both directions, and persists an upgrade-required
  state until that peer upgrades to `1.1` and is authenticated again. Offline,
  transport, identity, and data failures remain distinct.

- 2026-08-12: Added a Chinese scenario flow comment before every membership topology test. Each comment states device order, any explicit acceptance or rejection, and the behavior the case proves; no runtime behavior changed.

- 2026-08-12: The diagnostic rerun located the sponsor-offline failure before C could save the third history event, while external relay DNS lookup was unavailable. A strict local-direct rerun failed at the same pre-recovery stage, so it is not a valid substitute for the topology. Restored the production-style relay topology unchanged; the real multi-device acceptance remains blocked by this workspace's address-discovery environment.

- 2026-08-12: Added stage-specific public-state diagnostics to the three-device sponsor-offline topology for both its first recovery and its restart recovery. The prior serial failure used a generic timeout, so no production conclusion can be drawn until this rerun identifies the exact stage.

- 2026-08-12: The serial topology run completed 12 passed, 2 failed, 1 ignored. Both failures were relay-dependent recovery paths and logs showed the environment could not resolve `relay.iroh.network`, while a public DNS endpoint remained reachable. The production-style relay topology remains intact; the next rerun must identify the exact waiting stage before deciding whether an environment-independent local topology belongs alongside it.

- 2026-08-12: The first complete serial rerun still failed the four-device relay topology after its history preconditions had passed, but its generic timeout did not reveal which later connectivity stage failed. Replaced the two remaining anonymous waits with stage diagnostics that report only public workspace summaries and observed device count; no production behavior changed.

- 2026-08-12: Strengthened the four-device relay topology's receiver-side proof before each sponsor leaves: B saves A/B history before A stops, B and C both save A/B/C history before B stops, and C/D must share the complete four-member history before A recovers. This is test-only sequencing that prevents a later admission from using a merely locally completed predecessor.

- 2026-08-12: Corrected three ADR-020 topology test assumptions without changing production behavior: C now joins through the still-online B after A is down; C explicitly accepts A's first removal before the recovered-state scenario expects the A/C branch to match; B explicitly rejects alongside D in the concurrent-decision scenario before their retained branch is expected to exchange content. The architecture maintenance record states that this is test-only alignment.

- 2026-08-12: The four-engine concurrent-decision topology still timed out after both local
  decisions completed. Replaced its anonymous waits with two stage-specific test helpers that
  report the non-content workspace summaries for A/B/C/D on timeout, so the next run can identify
  whether delivery or final branch-state propagation is missing. No production behavior changed.
- 2026-08-12: The new stage diagnostics showed the concurrent scenario reached its expected
  A/C-consistent and A/D-diverged state; the timeout instead occurred while waiting for D-to-B
  content delivery. Extended the shared delivery assertion to report only sender and receiver
  public workspace summaries on timeout before determining whether B is pending or removed.
- 2026-08-12: A rerun eventually delivered D-to-B content after the concurrent decisions, but
  only near the timeout. The topology now establishes B/D bidirectional content delivery before
  the removal, so its post-decision assertion proves an already usable unaffected relationship
  remains usable instead of measuring first-connection timing.
- 2026-08-12: Re-running the restart-pending topology exposed that B can independently receive
  the same removal and must decide before C/B branch communication is expected. Updated both
  rejection topologies so C and B explicitly reject the same event; their post-decision content
  assertion now proves the shared retained branch rather than relying on B's delivery timing.
- 2026-08-12: The existing long-offline stale-sponsor topology timed out only after B/C returned
  and D was expected to restore its current connections. Replaced that anonymous wait with a
  stage-specific failure report covering public summaries and D's peer list; no production
  behavior changed pending the next diagnostic result.
- 2026-08-12: The first diagnostic build failed because the peer list was consumed before it
  could be included in the timeout report. Kept the list borrowed while deriving peer identifiers;
  this is test-only compilation cleanup, not a runtime change.
- 2026-08-12: The second long-offline run showed the timeout occurs earlier, when D is expected
  to observe only B/C immediately after joining their newer branch. Replaced that anonymous
  connection-list wait with the same four-summary diagnostic to distinguish a transient peer
  refresh from a stale-state admission regression.
- 2026-08-12: The long-offline timeout was actually before D joined: its older setup assumed B
  automatically applied C's removal of offline A. Under ADR-020 B must decide, so the topology
  now waits for B's pending item and explicitly accepts it before asserting the B/C branch.
- 2026-08-12: The next timeout was from an unrelated immediate-full-connectivity precondition
  after D joined the new branch. The long-offline scenario now verifies the C/D state needed for
  the stale-sponsor rejection, then verifies B/C/D state equality after B and C return; connection
  refresh timing is no longer treated as a membership-state requirement.

- 2026-08-13: Began a requirement-by-requirement completion audit instead of treating the
  completed 0.19-to-1.1 upgrade scenario as proof of all ADR-020 behavior. The first serial
  topology run executed 14 ordinary cases and all failed at the same pre-network startup check.
- 2026-08-13: Added a red regression for a fresh installation whose private data root does not
  exist yet. It failed with the same v0.19 inspection error. The directory adopter now treats
  only `NotFound` as a fresh installation; the focused regression passed and the original
  completed-removal topology passed in 17.18 seconds.
- 2026-08-13: Ran all 17 topology cases serially. Nine passed, five failed, and three were skipped
  in 594.24 seconds. The failures are now separated into offline history handoff, reject-branch
  content gating, fresh-instance rejoin admission, and shutdown/relay categories; ADR-020 remains
  active and is not being reported complete.

- 2026-08-12: Reopened the ADR-020 implementation follow-up because its final topology matrix was
  still pending despite the implementation checklist being marked complete. Added a real
  three-engine red test for a pending removal that must survive C's restart before rejection;
  it also requires C and unaffected B to communicate in both directions after the rejection.
- 2026-08-12: Extended the existing three-engine rejection topology to prove that both directions
  of ordinary content are blocked across the diverged A-C relationship while unaffected B-C
  communication remains available. Added a four-engine red test for simultaneous opposite
  decisions on the same removal: C accepts while D rejects, leaving A-C consistent and A-D
  diverged with both branches still usable internally.

- 2026-08-12: Read repository instructions and the complete `to-spec` and `planning-with-files` skills.
- 2026-08-12: Inspected ADR-020, affected ADR/spec references, architecture-bible references, and worktree status.
- 2026-08-12: Defined completion criteria and recorded the accepted product/security trade-off.
- 2026-08-12: Replaced ADR-020 with an implementation-ready signed-branch, conflict-stop, and new-Space recovery design; renamed the file to match the decision.
- 2026-08-12: Updated direct references, marked conflicting ADR/spec clauses superseded, and synchronized the architecture bible's current model and maintenance record.
- 2026-08-12: Passed Cargo metadata, full workspace check, Rust formatting, architecture preflight, stale-link scan, and diff formatting checks. Full compile emitted only existing warnings.
- 2026-08-12: User approved a second revision: per-device confirmation for unseen removals, same-Space reconciliation after acceptance, and continuing isolated branches after rejection.
- 2026-08-12: Rewrote ADR-020 around bounded online reconciliation, dual known/applied history heads, signed removal decisions, and independent presence/membership/history states.
- 2026-08-12: Updated ADR/spec status notices, README index, and architecture-bible current sections; retained prior choices only as explicitly historical records.
- 2026-08-12: Completed consistency audit; clarified that a removal event is the author's own acceptance and only receiving members create decision records.
- 2026-08-12: Final revision passed Cargo metadata, full workspace check, Rust formatting, architecture preflight, stale-link scan, and diff formatting checks.
- 2026-08-12: Started the product implementation. Added an independently tested signed
  membership-history model with unique parent events, separate known/applied heads, durable
  user decisions, and rejection of mismatched accepted results. The existing runtime still uses
  auto-applied removal notices, so network, persistence, content gating, Engine APIs, and full
  topology verification remain outstanding.
- 2026-08-12: Persisted per-peer history relationships in the encrypted workspace state and
  connected `PendingRemovalDecision`, `Diverged`, and `Invalid` to the existing content-send
  gate without changing membership or presence. Core, application, and encrypted restart tests
  pass. The new member-history protocol does not yet populate those relationships in production.
- 2026-08-12: Extended the unified workspace snapshot, Engine result, UniFFI, and HarmonyOS
  binding to report pending-removal and diverged peer device lists separately from waiting and
  removed state. Focused core, Engine, UniFFI, and HarmonyOS mapping tests pass. The lists are
  currently populated only by the persisted relationship state; the history exchange and user
  decision action remain outstanding.
- 2026-08-12: Bound every signed `AddDevice` history event to both its member instance and stable
  device identifier. The binding participates in the event identity, so later state recovery can
  reconstruct the member-to-device relationship without trusting a mutable side mapping. Core
  membership-history tests pass; runtime persistence and replacement of the old chain remain.
- 2026-08-12: Added the local `MembershipReconciliation` to encrypted workspace convergence
  state. A local admission or re-admission creates the history for its current member instance,
  so received history and decisions now have a durable destination. Existing admission and
  removal flows have not yet written their events to this state.
- 2026-08-12: Made an applied membership history the preferred source for effective members and
  added history-derived member-instance-to-device lookup. This protects the future replacement
  path from relying on mutable device mappings after a pending removal. Existing history is still
  not populated by admission or removal production flows.
- 2026-08-12: Exposed the sole user decision through the Engine, UniFFI, HarmonyOS, and the
  mobile probe. The shared snapshot now includes the opaque pending removal event identifier,
  so a client can submit only that identifier and Accept or Reject; the convergence owner still
  derives, persists, signs, and sends the resulting decision. Focused Engine and UniFFI checks
  pass; full binding and topology validation remain pending.
- 2026-08-12: Replaced the active local-removal submission path with one signed membership-history
  event. It no longer writes or propagates the superseded auto-applied removal intent, so another
  device can only observe it through history reconciliation and must make its own decision.
  The focused admission/removal history test and full workspace compile pass. Old protocol types,
  recovery handoff, and their tests remain to be removed or redesigned.
- 2026-08-12: Removed the old automatic-removal reconciliation loop from the workspace runtime.
  An authenticated peer becoming reachable now triggers only the bounded membership-history
  exchange; timer, resume, and generic wakeups cannot revive the superseded intent, notice, or
  recovery propagation. The old protocol implementations are now unreachable production code and
  remain explicitly queued for deletion; ADR-020 is still not complete.
- 2026-08-12: Added a direct member-history exchange boundary and changed the convergence owner,
  Engine assembly, and test assemblies to use it. The existing iroh adapter still wraps the new
  message in the old envelope while its handler is being replaced; old network variants therefore
  remain removable work, not a compatible product path.
- 2026-08-12: Replaced the installed Iroh member-removal listeners with one authenticated,
  bounded member-history listener. Engine startup and its host check now require that listener;
  the old exchange, late-submission, and notice listeners are no longer installed. Their remaining
  adapter and state code is internal deletion work, not a reachable network fallback.
- 2026-08-12: Added and ran the Engine startup guard that proves the member-history listener is
  reachable while all three superseded removal listeners are absent. The focused host test passed.
  The old recovery and removal state still supports admission internals, so deleting it requires
  replacing that admission chain as one change rather than removing isolated fields.
- 2026-08-12: Re-ran focused application cases for a received removal waiting for this device and
  rejection isolating only the relevant peer; both passed. Updated the architecture maintenance
  record with the startup exclusion guard. Full repository checks are next.
- 2026-08-12: Replaced invitation invalidation's obsolete removal-intent count with the verified
  membership-history progress (retaining the existing change count only while a Space has no
  history yet). Added a failing regression test first; it now passes. Updated the architecture
  maintenance record and will repeat full checks after this behavior change.
- 2026-08-12: Migrated the affected removal validation test to a real saved membership-history
  state and asserted that failed requests leave that state unchanged. The ten convergence-owner
  tests, full workspace check, formatting, architecture preflight, and diff check now pass.
- 2026-08-12: Resumed deletion of the superseded removal path. Restored the accidentally removed
  membership-history test builder before continuing, because nine focused test compilation errors
  were all missing helper calls rather than product behavior failures.
- 2026-08-12: Removed the old recovery dependency from the convergence owner and Engine assembly.
  A missing explicit member instance now resolves only from the encrypted local convergence state;
  the focused owner test saves readiness first and all nine focused cases pass. Deleted the unused
  application test doubles for the old exchange, late submission, notice, verification, and recovery
  ports. The remaining old change chain, core state, and Iroh adapter are still queued as one replacement.
- 2026-08-12: Changed sponsor admission commit to append only signed membership-history events.
  It no longer writes a `WorkspaceChange` or pending handoff record; the returned admission progress
  is derived from the applied history. Added a red-green regression that now proves the old collections
  stay empty after admission. The focused admission-history test passes; old confirmation and waiting
  state remain separate replacement work.
- 2026-08-12: Renamed the admission reply to saved member-history facts and removed the joiner's old
  local confirmation write. The joiner still saves the sponsor facts before reporting success, while
  its applied member-history branch remains unchanged. Online presence now starts only bounded history
  reconciliation and offline presence no longer mutates workspace state. Focused admission tests and
  Engine compilation pass; the old confirmation, handoff, and waiting state remains queued for deletion.
- 2026-08-12: Deleted the application entry points that wrote member reachability and waiting state.
  Presence is no longer an input to workspace membership state; only online history reconciliation
  remains. The legacy core fields and state-machine variants are still queued for their shared deletion.
- 2026-08-12: Deleted the old Iroh removal-exchange adapter, its three protocol listeners, and the
  unused node construction methods. Membership history now has its own authenticated bounded Iroh
  transport with no legacy-envelope fallback. The old core protocol types remain queued for deletion.
- 2026-08-12: Deleted the application owner APIs for legacy workspace changes, confirmations,
  handoff batches, and pending confirmations, together with their isolated tests. Application
  membership facts now flow only through signed history; core persistence cleanup is next.
- 2026-08-12: Removed the application fallback reads of legacy change counts and removal records.
  Invitation generation and admission validation now read only signed membership history.
- 2026-08-12: Deleted the superseded core change, confirmation, handoff, intent, notice, and
  recovery contracts together with their Iroh and security implementations. Member instances now
  belong to the member-history model, and the shared public snapshot exposes history event count,
  effective members, pending local decisions, and relationship-scoped divergence without legacy
  waiting or intent counts. Updated all binding mappings and removed obsolete test doubles.
- 2026-08-12: Added a red-green regression for an admission received after an unresolved removal.
  The content gate now uses only the locally applied history, so that not-yet-adopted admission
  cannot be mistaken for a removed device. Updated the Engine interface description and the
  architecture overview to describe only the signed-history model; old public fields and protocol
  names no longer appear outside explicitly historical maintenance entries.
- 2026-08-12: Re-ran repository verification: workspace check, metadata, formatting, architecture
  preflight, diff check, core membership-history tests, application convergence tests, Engine public
  contract tests, UniFFI, HarmonyOS, and mobile probe tests all passed. The workspace check retains
  pre-existing unused-code warnings. Real multi-device topology acceptance was not run and remains
  explicitly pending.
- 2026-08-13: Resumed the five failures from the complete topology matrix. The concurrent
  accept/reject case failed alone in 58.82s at the same D-to-B receiver rejection after the expected
  branch state had already converged. Added failure-only diagnostics for the send report, both
  convergence summaries, and both rosters before changing product behavior.
- 2026-08-13: Failure-only diagnostics showed B and D had the same four-member digest and did not
  mark each other diverged, but B still kept D in `PendingRemovalDecision`. The decision was sent
  only to removal author A, so peers that relayed the same pending event could never compare their
  choices. Added red tests requiring bounded notification to pending peers and treating matching
  peer decisions as one branch.
- 2026-08-13: The first four-device rerun exposed order-dependent notification when recipients
  were selected only from relationships already marked pending, so decisions now go to every
  effective device on the sender's applied branch. The next rerun reached the correct symmetric
  split (A/C versus B/D); its remaining timeout was an outdated assertion that checked divergence
  only around A and omitted C's cross-branch relationships.
- 2026-08-13: Added a red test proving an accepting device must still notify the removed target;
  captured recipients before applying the decision. Three focused owner tests passed, then the
  complete concurrent accept/reject topology passed in 13.18s with receiver-confirmed content on
  both branches and blocked cross-branch sends.
- 2026-08-13: Reproduced fresh-instance rejoin as an immediate sponsor rejection rather than a
  network timeout. Moved the duplicate-device decision to the sponsor's applied branch: an active
  instance is rejected, while a stable device whose old instance is removed may join with a new
  instance. Removed the joiner's stale same-Space boolean check. Both focused sponsor decisions
  now pass; the full receiver-confirmed rejoin topology is next.
- 2026-08-13: The full rejoin exposed a second defect: member signature verification selected the
  first credential for a stable device, so an old and new instance with the same device identifier
  made the new instance's authenticated hello fail. Bound membership-history signatures to the
  exact member instance and stopped redundantly rechecking a historical admission signature after
  the signed event author had already covered it. The focused replay test and the full rejoin
  topology now pass; the latter confirmed C-to-A content in A's received history in 9.88 seconds.
- 2026-08-13: Fixed the final stale-sponsor shutdown failure. A peer-query future abandoned by a
  test select branch kept its Engine in-flight registration, so shutdown spent the entire deadline
  waiting before runtime teardown began. Registrations now unregister on future drop. The focused
  red-green regression passed, the original three-device topology passed with all engines shut down,
  and the complete serial matrix finished with 15 passed, 0 failed, and 2 official-v0.19 fixture
  cases skipped in 431.74 seconds.
