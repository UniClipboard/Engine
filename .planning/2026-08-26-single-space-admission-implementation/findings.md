# Findings

- `crates/uc-core/src/membership/admission.rs` already exists for `PeerAdmissionPort`; the new protocol module is named `space_admission` to avoid a Rust module collision.
- `SpaceAdmissionId` has redacted Debug and read-only bytes; the next invariant is rejecting the all-zero sentinel.
- Core must receive generated randomness from callers; it does not provide `new()` random generation.
- The current focused uc-core test loop is healthy, though shared artifact locks can add wait time.
- `SpaceAdmissionId`, `JoinId`, `AdmissionMessageId`, `InvitationId`, and `AdmissionChannelPeerId` share identical non-zero, byte-access, ordering, copy, and redaction rules; a private macro removes real duplication without collapsing the distinct public types.
- Rust 1.95 cannot use array `PartialEq` in this fallible `const fn`; constructors are ordinary `fn`, while read-only `as_bytes` remains const.
- Moving `space_admission.rs` to `space_admission/mod.rs` did not improve locality by itself: the file still combined seven responsibilities and 38 tests across 2,830 lines.
- The intended external seam remains `membership::space_admission`; the split is private and must not change the existing re-export list in `membership/mod.rs`.
- Core callers now receive `AdmissionTransition` with replacement, exact reply, and one-time effects; aggregate views expose terminal status and pending exchange without exporting role/stage types.
- Legacy `SpaceJoinRecord` cannot be physically deleted during the Core phase because the not-yet-cut-over Application still consumes it; Spec 028 requires that deletion after Application/ledger replacement, before publication.
- The first Application tracer test must observe the complete two-node behavior rather than the old prepare/store cases: local Pending after saving JoinRequest, Sponsor save-before-Candidate, Joiner save-before-Prepared, and exact Candidate replay without a second Sponsor commit.
