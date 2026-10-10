fn work_of(sponsor: &SponsorAdmission) -> AdmissionOutstandingWork {
    let encoded = sponsor.encode_persisted().expect("record encodes");
    SpaceAdmissionAggregate::decode_persisted(&encoded)
        .expect("record decodes")
        .outstanding_work()
}

fn legacy_sponsor(aggregate: SpaceAdmissionAggregate) -> SponsorAdmission {
    SponsorAdmission::try_from_record(aggregate.into_legacy_persistence_fixture())
        .expect("legacy Sponsor fixture")
}

#[test]
fn legacy_sponsor_records_have_no_deadline_but_still_block_new_admission() {
    for aggregate in [
        sponsor_candidate_aggregate_fixture(),
        sponsor_committed_aggregate_fixture(),
        sponsor_applied_with_deadline_fixture(),
    ] {
        let legacy = legacy_sponsor(aggregate);
        let work = work_of(&legacy);

        assert!(legacy.is_legacy_unbounded());
        assert!(work.blocks_new_admission());
        assert_eq!(work.next_step(), None);
        assert_eq!(work.deadline_ms(), None);
        assert!(work.missing_deadline());
        assert!(legacy
            .terminate_if_expired(i64::MAX - 1)
            .expect("expiry check")
            .is_none());
    }
}

#[test]
fn records_with_a_deadline_are_not_legacy_unbounded() {
    let current = SponsorAdmission::try_from_record(sponsor_candidate_aggregate_fixture())
        .expect("current Sponsor fixture");

    assert!(!current.is_legacy_unbounded());
    assert!(current
        .close_legacy(LegacySponsorMembership::Absent)
        .expect("not legacy")
        .is_none());
}

#[test]
fn legacy_candidate_closes_without_a_member_and_stops_blocking() {
    let legacy = legacy_sponsor(sponsor_candidate_aggregate_fixture());
    let version_before = legacy.record_version();
    assert!(legacy.legacy_member_query().expect("query").is_none());

    let closed = legacy
        .close_legacy(LegacySponsorMembership::Absent)
        .expect("legacy Candidate closes")
        .expect("legacy Candidate is selected")
        .into_replacement();

    assert_eq!(closed.record_version(), version_before + 1);
    assert!(closed.pairing_confirmation().is_none());
    let work = work_of(&closed);
    assert!(work.is_settled());
    assert!(!work.blocks_new_admission());
    assert!(!work.holds_pairing_open());
    assert!(!work.missing_deadline());
    assert_eq!(work.next_step(), None);
}

#[test]
fn legacy_candidate_rejects_contradictory_member_evidence() {
    let legacy = legacy_sponsor(sponsor_candidate_aggregate_fixture());

    assert!(legacy.close_legacy(LegacySponsorMembership::Present).is_err());
}

#[test]
fn legacy_committed_and_applied_close_according_to_the_ledger_evidence() {
    for aggregate in [
        sponsor_committed_aggregate_fixture,
        sponsor_applied_with_deadline_fixture,
    ] {
        let absent = legacy_sponsor(aggregate())
            .close_legacy(LegacySponsorMembership::Absent)
            .expect("absent evidence closes")
            .expect("legacy record is selected")
            .into_replacement();
        assert!(absent.pairing_confirmation().is_none());
        assert!(!work_of(&absent).blocks_new_admission());

        let legacy = legacy_sponsor(aggregate());
        let query = legacy
            .legacy_member_query()
            .expect("query")
            .expect("Committed and Applied may have written the ledger");
        let present = legacy
            .close_legacy(LegacySponsorMembership::Present)
            .expect("present evidence closes")
            .expect("legacy record is selected")
            .into_replacement();
        let confirmation = present
            .pairing_confirmation()
            .expect("a committed member keeps an unconfirmed summary");
        assert_eq!(
            confirmation.status(),
            SponsorPairingConfirmationStatus::Unconfirmed
        );
        assert_eq!(confirmation.member_instance_id(), query.member_instance_id());
        assert_eq!(confirmation.add_event_id(), query.add_event_id());
        assert!(!work_of(&present).blocks_new_admission());
        assert!(!work_of(&present).holds_pairing_open());
    }
}

#[test]
fn closed_legacy_sponsor_records_survive_persistence_and_never_settle() {
    for membership in [
        LegacySponsorMembership::Absent,
        LegacySponsorMembership::Present,
    ] {
        let closed = legacy_sponsor(sponsor_applied_with_deadline_fixture())
            .close_legacy(membership)
            .expect("closes")
            .expect("selected")
            .into_replacement();
        let encoded = closed.encode_persisted().expect("closed record encodes");
        let reopened = SponsorAdmission::decode_persisted(&encoded).expect("closed record decodes");

        assert_eq!(reopened.record_version(), closed.record_version());
        assert_eq!(
            reopened.pairing_confirmation().map(|summary| summary.status()),
            closed.pairing_confirmation().map(|summary| summary.status())
        );
        assert!(work_of(&reopened).is_settled());
        assert!(reopened
            .close_legacy(membership)
            .expect("already closed")
            .is_none());
    }
}
