//! 本机历史标签操作。

use crate::error_codes::*;

use uc_application::facade::clipboard_history::{
    HistoryEntryTagSummaryView, HistoryTagBatchView, HistoryTagError, HistoryTagRenameView,
    HistoryTagView,
};
use uc_application::facade::AppFacade;
use uc_observability_contract::uc_error;

use crate::{
    CreateHistoryTagInput, EngineError, EngineErrorCategory, HistoryEntryTagSummary,
    HistoryEntryTagsInput, HistoryTagApplicationSummary, HistoryTagBatchSummary,
    HistoryTagCreatedSummary, HistoryTagDeletedSummary, HistoryTagEntriesInput, HistoryTagInput,
    HistoryTagMergeSummary, HistoryTagRenameSummary, HistoryTagSummary, MergeHistoryTagsInput,
    OperationResult, RenameHistoryTagInput,
};

pub async fn execute_list_history_tags(facade: &AppFacade) -> Result<OperationResult, EngineError> {
    let tags = facade.list_history_tags().await.map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTags(
        tags.into_iter().map(tag_summary).collect(),
    ))
}

pub async fn execute_create_history_tag(
    facade: &AppFacade,
    input: CreateHistoryTagInput,
) -> Result<OperationResult, EngineError> {
    let created = facade
        .create_history_tag(&input.name)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTagCreated(
        HistoryTagCreatedSummary {
            tag: tag_summary(created.tag),
            created: created.created,
        },
    ))
}

pub async fn execute_rename_history_tag(
    facade: &AppFacade,
    input: RenameHistoryTagInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.tag_id)?;
    let renamed = facade
        .rename_history_tag(&input.tag_id, &input.name)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTagRenamed(match renamed {
        HistoryTagRenameView::Renamed(tag) => HistoryTagRenameSummary::Renamed(tag_summary(tag)),
        HistoryTagRenameView::NameConflict { existing_tag_id } => {
            HistoryTagRenameSummary::NameConflict { existing_tag_id }
        }
    }))
}

pub async fn execute_add_history_tag_to_entries(
    facade: &AppFacade,
    input: HistoryTagEntriesInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.tag_id)?;
    validate_ids(&input.entry_ids)?;
    let batch = facade
        .add_history_tag_to_entries(&input.tag_id, &input.entry_ids)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTagEntriesChanged(batch_summary(
        batch,
    )))
}

pub async fn execute_remove_history_tag_from_entries(
    facade: &AppFacade,
    input: HistoryTagEntriesInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.tag_id)?;
    validate_ids(&input.entry_ids)?;
    let batch = facade
        .remove_history_tag_from_entries(&input.tag_id, &input.entry_ids)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTagEntriesChanged(batch_summary(
        batch,
    )))
}

pub async fn execute_summarize_history_entry_tags(
    facade: &AppFacade,
    input: HistoryEntryTagsInput,
) -> Result<OperationResult, EngineError> {
    validate_ids(&input.entry_ids)?;
    let summary = facade
        .summarize_history_entry_tags(&input.entry_ids)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryEntryTags(entry_tag_summary(
        summary,
    )))
}

pub async fn execute_merge_history_tags(
    facade: &AppFacade,
    input: MergeHistoryTagsInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.target_tag_id)?;
    validate_ids(&input.source_tag_ids)?;
    let merged = facade
        .merge_history_tags(&input.source_tag_ids, &input.target_tag_id)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTagsMerged(HistoryTagMergeSummary {
        moved: merged.moved,
        already_on_target: merged.already_on_target,
    }))
}

pub async fn execute_delete_history_tag(
    facade: &AppFacade,
    input: HistoryTagInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.tag_id)?;
    let detached = facade
        .delete_history_tag(&input.tag_id)
        .await
        .map_err(map_tag_error)?;
    Ok(OperationResult::HistoryTagDeleted(
        HistoryTagDeletedSummary { detached },
    ))
}

fn tag_summary(tag: HistoryTagView) -> HistoryTagSummary {
    HistoryTagSummary {
        tag_id: tag.tag_id,
        name: tag.name,
        created_at_ms: tag.created_at_ms,
        entry_count: tag.entry_count,
    }
}

fn batch_summary(batch: HistoryTagBatchView) -> HistoryTagBatchSummary {
    HistoryTagBatchSummary {
        changed: batch.changed,
        unchanged: batch.unchanged,
        missing_entry_ids: batch.missing_entry_ids,
    }
}

fn entry_tag_summary(summary: HistoryEntryTagSummaryView) -> HistoryEntryTagSummary {
    HistoryEntryTagSummary {
        selected: summary.selected,
        tags: summary
            .tags
            .into_iter()
            .map(|tag| HistoryTagApplicationSummary {
                tag_id: tag.tag_id,
                applied: tag.applied,
            })
            .collect(),
    }
}

fn validate_id(id: &str) -> Result<(), EngineError> {
    if id.trim().is_empty() {
        return Err(invalid_input_error());
    }
    Ok(())
}

fn validate_ids(ids: &[String]) -> Result<(), EngineError> {
    ids.iter().try_for_each(|id| validate_id(id))
}

fn invalid_input_error() -> EngineError {
    EngineError::new(
        HISTORY_INVALID_INPUT_CODE,
        EngineErrorCategory::InvalidInput,
        false,
    )
}

fn map_tag_error(error: HistoryTagError) -> EngineError {
    match error {
        HistoryTagError::InvalidName(_) | HistoryTagError::InvalidInput => invalid_input_error(),
        HistoryTagError::NotFound => {
            EngineError::new(HISTORY_NOT_FOUND_CODE, EngineErrorCategory::NotFound, false)
        }
        HistoryTagError::Locked => EngineError::new(
            HISTORY_TAGS_LOCKED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        HistoryTagError::Unavailable => EngineError::new(
            HISTORY_TAGS_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
        ),
        HistoryTagError::Internal(_) => {
            uc_error!("history tag operation failed");
            EngineError::new(HISTORY_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
    }
}
