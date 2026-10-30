use super::{
    ProgressSnapshotCoverage, ProgressSnapshotEffect as Effect, ProgressSnapshotInput as Input,
    ProgressSnapshotRefusal as Refusal,
};
use crate::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, RegistrationDescriptor,
};

fn descriptor(count: u16) -> RegistrationDescriptor<ProgressIdentity> {
    RegistrationDescriptor {
        producer: ProgressIdentity::new([1; 16]),
        revision: 7,
        source: ProgressIdentity::new([2; 16]),
        attempt: ProgressIdentity::new([3; 16]),
        item_count: count,
    }
}
fn plan(count: u16) -> Vec<ProgressItem> {
    (0..count)
        .map(|index| ProgressItem {
            kind: ProgressItemKind::File,
            index,
        })
        .collect()
}
fn page(offset: u16, count: u16) -> Input {
    Input::Page {
        descriptor: descriptor(1024),
        revision: 9,
        offset,
        total: 1024,
        items: plan(1024)[usize::from(offset)..usize::from(offset + count)].to_vec(),
    }
}
#[test]
fn maximum_original_plan_is_complete_only_after_all_sixteen_ordered_pages() {
    let mut coverage = ProgressSnapshotCoverage::new(descriptor(1024), plan(1024)).unwrap();
    for offset in (0..1024).step_by(64) {
        assert_eq!(
            coverage.apply(page(offset, 64)),
            if offset == 960 {
                Effect::Complete { revision: 9 }
            } else {
                Effect::Accepted {
                    next_offset: offset + 64,
                    revision: 9,
                }
            }
        );
    }
    assert_eq!(
        coverage.apply(page(960, 64)),
        Effect::Refused(Refusal::Sealed)
    );
}
#[test]
fn missing_duplicate_skipped_and_mixed_revision_pages_never_complete_or_reopen() {
    let mut cases = vec![
        (Input::End, Refusal::Incomplete),
        (page(0, 64), Refusal::WrongOffset),
        (page(128, 64), Refusal::WrongOffset),
    ];
    let mut changed = page(64, 64);
    if let Input::Page { revision, .. } = &mut changed {
        *revision = 10;
    }
    cases.push((changed, Refusal::RevisionChanged));
    for (input, reason) in cases {
        let mut coverage = ProgressSnapshotCoverage::new(descriptor(1024), plan(1024)).unwrap();
        assert!(matches!(
            coverage.apply(page(0, 64)),
            Effect::Accepted { .. }
        ));
        assert_eq!(coverage.apply(input), Effect::Refused(reason));
        assert_eq!(
            coverage.apply(page(64, 64)),
            Effect::Refused(Refusal::Sealed)
        );
    }
}
#[test]
fn current_descriptor_is_exact_and_item_kind_and_original_position_are_authoritative() {
    let mut cases = Vec::new();
    for field in 0..6 {
        let mut changed = page(0, 64);
        if let Input::Page { descriptor, .. } = &mut changed {
            match field {
                0 => descriptor.producer = ProgressIdentity::new([8; 16]),
                1 => descriptor.source = ProgressIdentity::new([8; 16]),
                2 => descriptor.attempt = ProgressIdentity::new([8; 16]),
                3 => descriptor.revision += 1,
                4 => descriptor.item_count -= 1,
                _ => descriptor.revision -= 1,
            }
        }
        cases.push((changed, Refusal::DescriptorChanged));
    }
    let mut changed = page(0, 64);
    if let Input::Page { items, .. } = &mut changed {
        items[0].kind = ProgressItemKind::Representation;
    }
    cases.push((changed, Refusal::WrongItem));
    let mut changed = page(0, 64);
    if let Input::Page { items, .. } = &mut changed {
        items.swap(0, 1);
    }
    cases.push((changed, Refusal::WrongItem));
    let mut changed = page(64, 64);
    if let Input::Page { items, offset, .. } = &mut changed {
        *offset = 0;
        items[0].index = 0;
    }
    cases.push((changed, Refusal::WrongItem));
    for (input, reason) in cases {
        let mut coverage = ProgressSnapshotCoverage::new(descriptor(1024), plan(1024)).unwrap();
        assert_eq!(coverage.apply(input), Effect::Refused(reason));
    }
}
#[test]
fn invalid_plan_total_and_page_capacity_are_refused_before_acceptance() {
    for (count, items) in [
        (0, Vec::new()),
        (1024, plan(1025)),
        (1, plan(2)),
        (2, vec![plan(1)[0]; 2]),
    ] {
        assert!(matches!(
            ProgressSnapshotCoverage::new(descriptor(count), items),
            Err(Refusal::InvalidPlan)
        ));
    }
    for (count, reason) in [(0, Refusal::PageSize), (65, Refusal::PageSize)] {
        let mut coverage = ProgressSnapshotCoverage::new(descriptor(1024), plan(1024)).unwrap();
        assert_eq!(coverage.apply(page(0, count)), Effect::Refused(reason));
    }
    let mut changed = page(0, 64);
    if let Input::Page { total, .. } = &mut changed {
        *total = 1000;
    }
    let mut coverage = ProgressSnapshotCoverage::new(descriptor(1024), plan(1024)).unwrap();
    assert_eq!(
        coverage.apply(changed),
        Effect::Refused(Refusal::WrongTotal)
    );
}
#[test]
fn sparse_representation_plan_and_directory_unit_follow_the_original_order() {
    for items in [
        vec![
            ProgressItem {
                kind: ProgressItemKind::Representation,
                index: 900,
            },
            ProgressItem {
                kind: ProgressItemKind::File,
                index: 0,
            },
        ],
        vec![ProgressItem {
            kind: ProgressItemKind::DirectoryUnit,
            index: 0,
        }],
    ] {
        let descriptor = descriptor(items.len() as u16);
        let mut coverage = ProgressSnapshotCoverage::new(descriptor, items.clone()).unwrap();
        assert_eq!(
            coverage.apply(Input::Page {
                descriptor,
                revision: 0,
                offset: 0,
                total: descriptor.item_count,
                items
            }),
            Effect::Complete { revision: 0 }
        );
    }
}

mod adoption;
