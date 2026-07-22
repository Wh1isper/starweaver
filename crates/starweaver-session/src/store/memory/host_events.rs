use std::collections::{BTreeMap, BTreeSet};

use crate::{
    DurableHostEventClass, DurableHostEventPage, DurableHostEventQuery, DurableHostEventRecord,
    DurableHostEventScope, EventPublicationKey, MAX_HOST_EVENT_PAGE_SIZE, MAX_HOST_EVENT_POSITION,
    PendingHostEventPublication, SessionStoreError, SessionStoreResult,
};

use super::{InMemorySessionStore, StoreInner, store_failed};

fn validate_batch_limit(limit: usize) -> SessionStoreResult<()> {
    if !(1..=MAX_HOST_EVENT_PAGE_SIZE).contains(&limit) {
        return Err(SessionStoreError::Failed(format!(
            "host event batch limit must be between 1 and {MAX_HOST_EVENT_PAGE_SIZE}"
        )));
    }
    Ok(())
}

fn publication_conflict(publication_key: &EventPublicationKey) -> SessionStoreError {
    SessionStoreError::Conflict(format!(
        "host event publication conflict for {}",
        publication_key.as_str()
    ))
}

fn pending_in_order(
    inner: &StoreInner,
    limit: usize,
) -> SessionStoreResult<Vec<PendingHostEventPublication>> {
    inner
        .host_event_outbox_order
        .values()
        .take(limit)
        .map(|publication_key| {
            inner
                .host_event_outbox
                .get(publication_key)
                .cloned()
                .ok_or_else(|| {
                    SessionStoreError::Failed(format!(
                        "host event outbox order is corrupt for {}",
                        publication_key.as_str()
                    ))
                })
        })
        .collect()
}

fn remove_pending(inner: &mut StoreInner, publication_key: &EventPublicationKey) {
    if let Some(sequence) = inner.host_event_outbox_sequences.remove(publication_key) {
        inner.host_event_outbox_order.remove(&sequence);
    }
    inner.host_event_outbox.remove(publication_key);
}

pub(super) fn enqueue_host_event_publications_locked(
    inner: &mut StoreInner,
    publications: &[PendingHostEventPublication],
) -> SessionStoreResult<()> {
    let mut batch_by_key = BTreeMap::new();
    let mut batch_event_ids = BTreeMap::new();
    for publication in publications {
        publication.validate()?;
        if let Some(existing) =
            batch_by_key.insert(publication.publication_key.clone(), publication.clone())
            && existing != *publication
        {
            return Err(publication_conflict(&publication.publication_key));
        }
        if let Some(existing_key) = batch_event_ids.insert(
            publication.event_id.clone(),
            publication.publication_key.clone(),
        ) && existing_key != publication.publication_key
        {
            return Err(SessionStoreError::Conflict(format!(
                "host event identity conflict for {}",
                publication.event_id
            )));
        }
    }

    for publication in batch_by_key.values() {
        if let Some(position) = inner.host_event_positions.get(&publication.publication_key) {
            let existing = inner.host_event_records.get(position).ok_or_else(|| {
                SessionStoreError::Failed(format!(
                    "host event position index is corrupt for {}",
                    publication.publication_key.as_str()
                ))
            })?;
            if existing.pending_projection() != *publication {
                return Err(publication_conflict(&publication.publication_key));
            }
        }
        if let Some(existing) = inner.host_event_outbox.get(&publication.publication_key)
            && existing != publication
        {
            return Err(publication_conflict(&publication.publication_key));
        }
        if let Some(existing_key) = inner.host_event_ids.get(&publication.event_id)
            && existing_key != &publication.publication_key
        {
            return Err(SessionStoreError::Conflict(format!(
                "host event identity conflict for {}",
                publication.event_id
            )));
        }
    }

    let mut inserted_keys = BTreeSet::new();
    for publication in publications {
        if !inserted_keys.insert(publication.publication_key.clone())
            || inner
                .host_event_positions
                .contains_key(&publication.publication_key)
            || inner
                .host_event_outbox
                .contains_key(&publication.publication_key)
        {
            continue;
        }
        let sequence = inner
            .last_host_event_outbox_sequence
            .checked_add(1)
            .filter(|sequence| *sequence <= MAX_HOST_EVENT_POSITION)
            .ok_or_else(|| {
                SessionStoreError::Failed(
                    "host event outbox sequence space is exhausted".to_string(),
                )
            })?;
        inner.last_host_event_outbox_sequence = sequence;
        inner.host_event_ids.insert(
            publication.event_id.clone(),
            publication.publication_key.clone(),
        );
        inner
            .host_event_outbox_order
            .insert(sequence, publication.publication_key.clone());
        inner
            .host_event_outbox_sequences
            .insert(publication.publication_key.clone(), sequence);
        inner
            .host_event_outbox
            .insert(publication.publication_key.clone(), publication.clone());
    }
    Ok(())
}

impl InMemorySessionStore {
    pub(super) fn enqueue_host_event_publication_batch(
        &self,
        publications: &[PendingHostEventPublication],
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let mut staged = inner.clone();
        enqueue_host_event_publications_locked(&mut staged, publications)?;
        *inner = staged;
        Ok(())
    }

    pub(super) fn pending_host_event_publication_batch(
        &self,
        limit: usize,
    ) -> SessionStoreResult<Vec<PendingHostEventPublication>> {
        validate_batch_limit(limit)?;
        let inner = self.inner.lock().map_err(store_failed)?;
        pending_in_order(&inner, limit)
    }

    pub(super) fn materialize_host_event_publication_batch(
        &self,
        limit: usize,
    ) -> SessionStoreResult<Vec<DurableHostEventRecord>> {
        validate_batch_limit(limit)?;
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let pending = pending_in_order(&inner, limit)?;
        let mut staged = inner.clone();
        let mut materialized = Vec::with_capacity(pending.len());
        for publication in pending {
            if let Some(position) = staged
                .host_event_positions
                .get(&publication.publication_key)
                .copied()
            {
                let existing = staged.host_event_records.get(&position).ok_or_else(|| {
                    SessionStoreError::Failed(format!(
                        "host event position index is corrupt for {}",
                        publication.publication_key.as_str()
                    ))
                })?;
                if existing.pending_projection() != publication {
                    return Err(publication_conflict(&publication.publication_key));
                }
                materialized.push(existing.clone());
                remove_pending(&mut staged, &publication.publication_key);
                continue;
            }
            let position = staged
                .last_host_event_position
                .checked_add(1)
                .filter(|position| *position <= MAX_HOST_EVENT_POSITION)
                .ok_or_else(|| {
                    SessionStoreError::Failed(
                        "durable host event position space is exhausted".to_string(),
                    )
                })?;
            let record = DurableHostEventRecord::from_pending(position, publication.clone());
            staged.last_host_event_position = position;
            staged
                .host_event_positions
                .insert(publication.publication_key.clone(), position);
            staged.host_event_ids.insert(
                publication.event_id.clone(),
                publication.publication_key.clone(),
            );
            staged.host_event_records.insert(position, record.clone());
            remove_pending(&mut staged, &publication.publication_key);
            materialized.push(record);
        }
        *inner = staged;
        Ok(materialized)
    }

    pub(super) fn replay_host_event_page(
        &self,
        query: DurableHostEventQuery,
    ) -> SessionStoreResult<DurableHostEventPage> {
        // Reconstructing through the checked constructor prevents callers inside this crate from
        // bypassing the public query invariants with a struct literal.
        let query = DurableHostEventQuery::new(
            query.scope,
            query.event_classes,
            query.after_position,
            query.limit,
        )?;
        let inner = self.inner.lock().map_err(store_failed)?;
        let after = query.after_position.unwrap_or(0);
        let mut eligible = inner
            .host_event_records
            .range((after.saturating_add(1))..)
            .map(|(_, record)| record)
            .filter(|record| {
                query.scope.contains(&record.scope)
                    && query.event_classes.contains(&record.event_class)
            })
            .take(query.limit.saturating_add(1))
            .cloned()
            .collect::<Vec<_>>();
        let has_more = eligible.len() > query.limit;
        if has_more {
            eligible.pop();
        }
        let next_position = eligible
            .last()
            .map(|record| record.position)
            .or(query.after_position);
        Ok(DurableHostEventPage {
            records: eligible,
            next_position,
            has_more,
        })
    }

    pub(super) fn host_event_fence_position(
        &self,
        scope: &DurableHostEventScope,
        event_classes: &[DurableHostEventClass],
    ) -> SessionStoreResult<Option<u64>> {
        let classes = event_classes.iter().copied().collect::<BTreeSet<_>>();
        if classes.is_empty() {
            return Err(SessionStoreError::Failed(
                "host event fence requires at least one event class".to_string(),
            ));
        }
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .host_event_records
            .iter()
            .rev()
            .find_map(|(position, record)| {
                (scope.contains(&record.scope) && classes.contains(&record.event_class))
                    .then_some(*position)
            }))
    }
}
