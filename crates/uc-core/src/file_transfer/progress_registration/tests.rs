use super::{ProgressRegistration, RegistrationDescriptor, RegistrationEffect as Effect};
use super::{RegistrationInput as Input, RegistrationRefusal as Refusal};

fn descriptor(producer: u8, revision: u64, attempt: u8) -> RegistrationDescriptor<u8> {
    RegistrationDescriptor {
        producer,
        revision,
        source: 1,
        attempt,
        item_count: 3,
    }
}
fn bind(
    owner: &mut ProgressRegistration<u8>,
    generation: u64,
    nonce: u8,
    descriptor: RegistrationDescriptor<u8>,
) -> Effect {
    assert_eq!(
        owner.apply(Input::Challenge { generation, nonce }),
        Effect::SendChallenge
    );
    owner.apply(Input::Describe {
        generation,
        nonce,
        binding: nonce,
        descriptor,
    })
}

#[test]
fn reconnect_retains_revision_and_restart_requires_a_new_connection() {
    let mut owner = ProgressRegistration::new(1);
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 1 }),
        Effect::ConnectionReplaced
    );
    assert_eq!(
        bind(&mut owner, 1, 10, descriptor(2, 8, 3)),
        Effect::SendBound
    );
    assert_eq!(
        bind(&mut owner, 1, 11, descriptor(4, 1, 5)),
        Effect::Refused(Refusal::ProducerChangedOnConnection)
    );
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 2 }),
        Effect::ConnectionReplaced
    );
    assert_eq!(
        bind(&mut owner, 2, 12, descriptor(2, 7, 3)),
        Effect::Refused(Refusal::RevisionRegressed)
    );
    assert_eq!(
        bind(&mut owner, 2, 13, descriptor(2, 8, 5)),
        Effect::Refused(Refusal::RevisionConflict)
    );
    assert_eq!(
        bind(&mut owner, 2, 14, descriptor(2, 8, 3)),
        Effect::SendBound
    );
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 3 }),
        Effect::ConnectionReplaced
    );
    assert_eq!(
        bind(&mut owner, 3, 15, descriptor(4, 1, 5)),
        Effect::SendBound
    );
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 2 }),
        Effect::Refused(Refusal::RetiredConnection)
    );
    assert_eq!(
        owner.apply(Input::Describe {
            generation: 2,
            nonce: 14,
            binding: 14,
            descriptor: descriptor(2, 8, 3)
        }),
        Effect::Refused(Refusal::RetiredConnection)
    );
}

#[test]
fn old_challenges_and_duplicate_descriptors_cannot_replace_a_new_attempt() {
    let mut owner = ProgressRegistration::new(1);
    owner.apply(Input::ReplaceConnection { generation: 1 });
    owner.apply(Input::Challenge {
        generation: 1,
        nonce: 10,
    });
    assert_eq!(
        bind(&mut owner, 1, 11, descriptor(2, 9, 4)),
        Effect::SendBound
    );
    for nonce in [10, 11] {
        assert_eq!(
            owner.apply(Input::Describe {
                generation: 1,
                nonce,
                binding: nonce,
                descriptor: descriptor(2, 8, 3)
            }),
            Effect::Refused(Refusal::StaleChallenge)
        );
    }
    for (generation, binding, source, attempt, revision, expected) in [
        (1, 11, 1, 4, 9, Effect::CheckNotification),
        (1, 10, 1, 4, 9, Effect::Refused(Refusal::WrongBinding)),
        (1, 11, 2, 4, 9, Effect::Refused(Refusal::WrongBinding)),
        (1, 11, 1, 3, 9, Effect::Refused(Refusal::WrongBinding)),
        (1, 11, 1, 4, 8, Effect::Refused(Refusal::WrongBinding)),
        (0, 11, 1, 4, 9, Effect::Refused(Refusal::RetiredConnection)),
    ] {
        assert_eq!(
            owner.apply(Input::Notification {
                generation,
                binding,
                source,
                attempt,
                revision
            }),
            expected
        );
    }
}

#[test]
fn invalid_source_and_empty_plan_never_bind_and_consume_current_challenge() {
    let mut owner = ProgressRegistration::new(1);
    assert_eq!(
        owner.apply(Input::Challenge {
            generation: 0,
            nonce: 9
        }),
        Effect::Refused(Refusal::RetiredConnection)
    );
    owner.apply(Input::ReplaceConnection { generation: 1 });
    let mut wrong = descriptor(2, 1, 3);
    wrong.source = 9;
    assert_eq!(
        bind(&mut owner, 1, 10, wrong),
        Effect::Refused(Refusal::WrongSource)
    );
    let mut empty = descriptor(2, 1, 3);
    empty.item_count = 0;
    assert_eq!(
        bind(&mut owner, 1, 11, empty),
        Effect::Refused(Refusal::EmptyPlan)
    );
    assert_eq!(
        owner.apply(Input::Describe {
            generation: 1,
            nonce: 11,
            binding: 11,
            descriptor: descriptor(2, 1, 3)
        }),
        Effect::Refused(Refusal::StaleChallenge)
    );
}

#[test]
fn initial_query_confirmation_does_not_bind_notifications_and_fences_older_pending_description() {
    let mut owner = ProgressRegistration::new(1);
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 1 }),
        Effect::ConnectionReplaced
    );
    assert_eq!(
        owner.apply(Input::Challenge {
            generation: 1,
            nonce: 10
        }),
        Effect::SendChallenge
    );
    let current = descriptor(2, 8, 3);
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: current
        }),
        Effect::QueryVerified
    );
    assert_eq!(
        owner.apply(Input::Notification {
            generation: 1,
            binding: 10,
            source: 1,
            attempt: 3,
            revision: 8
        }),
        Effect::Refused(Refusal::WrongBinding)
    );
    assert_eq!(
        owner.apply(Input::Describe {
            generation: 1,
            nonce: 10,
            binding: 10,
            descriptor: descriptor(2, 7, 3)
        }),
        Effect::Refused(Refusal::RevisionRegressed)
    );
    assert_eq!(bind(&mut owner, 1, 11, current), Effect::SendBound);
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: current
        }),
        Effect::QueryVerified
    );
    assert_eq!(
        owner.apply(Input::Notification {
            generation: 1,
            binding: 11,
            source: 1,
            attempt: 3,
            revision: 8
        }),
        Effect::CheckNotification
    );
}
#[test]
fn query_cannot_rewrite_a_known_scope_or_confirm_a_foreign_or_empty_original_plan() {
    let mut owner = ProgressRegistration::new(1);
    let current = descriptor(2, 8, 3);
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: RegistrationDescriptor {
                source: 9,
                ..current
            }
        }),
        Effect::Refused(Refusal::WrongSource)
    );
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: RegistrationDescriptor {
                item_count: 0,
                ..current
            }
        }),
        Effect::Refused(Refusal::EmptyPlan)
    );
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: current
        }),
        Effect::QueryVerified
    );
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: descriptor(4, 1, 5)
        }),
        Effect::Refused(Refusal::RevisionConflict)
    );
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 1 }),
        Effect::ConnectionReplaced
    );
    assert_eq!(
        bind(&mut owner, 1, 10, descriptor(2, 7, 3)),
        Effect::Refused(Refusal::RevisionRegressed)
    );
    assert_eq!(bind(&mut owner, 1, 11, current), Effect::SendBound);
}

#[test]
fn matching_pending_registration_can_finish_after_initial_query_confirmation() {
    let mut owner = ProgressRegistration::new(1);
    let current = descriptor(2, 8, 3);
    assert_eq!(
        owner.apply(Input::ReplaceConnection { generation: 1 }),
        Effect::ConnectionReplaced
    );
    assert_eq!(
        owner.apply(Input::Challenge {
            generation: 1,
            nonce: 10
        }),
        Effect::SendChallenge
    );
    assert_eq!(
        owner.apply(Input::QueryVerified {
            descriptor: current
        }),
        Effect::QueryVerified
    );
    assert_eq!(
        owner.apply(Input::Describe {
            generation: 1,
            nonce: 10,
            binding: 11,
            descriptor: current
        }),
        Effect::SendBound
    );
    assert_eq!(
        owner.apply(Input::Notification {
            generation: 1,
            binding: 11,
            source: 1,
            attempt: 3,
            revision: 8
        }),
        Effect::CheckNotification
    );
}
