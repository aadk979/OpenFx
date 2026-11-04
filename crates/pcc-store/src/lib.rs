//! Immutable scene snapshots, optimistic transactions, and monotonic undo/redo history.
use pcc_graph::GraphSnapshot;
use pcc_id::RevisionId;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Transaction {
    pub base_revision: RevisionId,
    pub read_set: Arc<[RevisionId]>,
}
#[derive(Debug, Eq, PartialEq)]
pub enum StoreError {
    Conflict {
        expected: RevisionId,
        actual: RevisionId,
    },
    NothingToUndo,
    NothingToRedo,
    Poisoned,
}

struct StoreState {
    current: Arc<GraphSnapshot>,
    undo_stack: Vec<Arc<GraphSnapshot>>,
    redo_stack: Vec<Arc<GraphSnapshot>>,
}

pub struct SceneStore {
    state: Mutex<StoreState>,
}

impl SceneStore {
    #[must_use]
    pub fn new(initial: GraphSnapshot) -> Self {
        Self {
            state: Mutex::new(StoreState {
                current: Arc::new(initial),
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
            }),
        }
    }
    pub fn snapshot(&self) -> Result<Arc<GraphSnapshot>, StoreError> {
        self.state
            .lock()
            .map(|state| Arc::clone(&state.current))
            .map_err(|_| StoreError::Poisoned)
    }
    pub fn begin(&self) -> Result<Transaction, StoreError> {
        let snapshot = self.snapshot()?;
        Ok(Transaction {
            base_revision: snapshot.revision,
            read_set: Arc::from([snapshot.revision]),
        })
    }
    pub fn commit(
        &self,
        transaction: &Transaction,
        mut next: GraphSnapshot,
    ) -> Result<Arc<GraphSnapshot>, StoreError> {
        let mut state = self.state.lock().map_err(|_| StoreError::Poisoned)?;
        if state.current.revision != transaction.base_revision
            || !transaction.read_set.contains(&state.current.revision)
        {
            return Err(StoreError::Conflict {
                expected: transaction.base_revision,
                actual: state.current.revision,
            });
        }
        next.revision = state.current.revision.next();
        let next = Arc::new(next);
        let prev = std::mem::replace(&mut state.current, Arc::clone(&next));
        state.undo_stack.push(prev);
        state.redo_stack.clear();
        Ok(next)
    }
    pub fn undo(&self, expected: RevisionId) -> Result<Arc<GraphSnapshot>, StoreError> {
        let mut state = self.state.lock().map_err(|_| StoreError::Poisoned)?;
        if state.current.revision != expected {
            return Err(StoreError::Conflict {
                expected,
                actual: state.current.revision,
            });
        }
        let Some(prev) = state.undo_stack.pop() else {
            return Err(StoreError::NothingToUndo);
        };
        let next_revision = state.current.revision.next();
        let restored = Arc::new(GraphSnapshot {
            revision: next_revision,
            nodes: prev.nodes.clone(),
        });
        let displaced = std::mem::replace(&mut state.current, Arc::clone(&restored));
        state.redo_stack.push(displaced);
        Ok(restored)
    }
    pub fn redo(&self, expected: RevisionId) -> Result<Arc<GraphSnapshot>, StoreError> {
        let mut state = self.state.lock().map_err(|_| StoreError::Poisoned)?;
        if state.current.revision != expected {
            return Err(StoreError::Conflict {
                expected,
                actual: state.current.revision,
            });
        }
        let Some(next_candidate) = state.redo_stack.pop() else {
            return Err(StoreError::NothingToRedo);
        };
        let next_revision = state.current.revision.next();
        let restored = Arc::new(GraphSnapshot {
            revision: next_revision,
            nodes: next_candidate.nodes.clone(),
        });
        let displaced = std::mem::replace(&mut state.current, Arc::clone(&restored));
        state.undo_stack.push(displaced);
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_math::Vec3;
    #[test]
    fn stale_transaction_cannot_publish() {
        let store = SceneStore::new(GraphSnapshot::demo(1, Vec3::ZERO));
        let first = store.begin();
        let second = store.begin();
        assert!(
            matches!(first,Ok(ref tx) if store.commit(tx,GraphSnapshot::demo(2,Vec3::ZERO)).is_ok())
        );
        assert!(
            matches!(second,Ok(ref tx) if matches!(store.commit(tx,GraphSnapshot::demo(3,Vec3::ZERO)),Err(StoreError::Conflict{..})))
        );
    }

    #[test]
    fn undo_and_redo_preserve_monotonic_revisions_and_conflict_checks() {
        let store = SceneStore::new(GraphSnapshot::demo(1, Vec3::ZERO));
        assert_eq!(store.undo(RevisionId(1)), Err(StoreError::NothingToUndo));

        let tx = store.begin();
        assert!(tx.is_ok());
        if let Ok(tx) = tx {
            let committed = store.commit(&tx, GraphSnapshot::demo(5, Vec3::ZERO));
            assert_eq!(committed.map(|s| s.revision), Ok(RevisionId(2)));
        }

        assert_eq!(
            store.undo(RevisionId(1)),
            Err(StoreError::Conflict {
                expected: RevisionId(1),
                actual: RevisionId(2),
            })
        );

        let undone = store.undo(RevisionId(2));
        assert_eq!(
            undone.as_ref().map(|s| (s.revision, &s.nodes)),
            Ok((RevisionId(3), &GraphSnapshot::demo(1, Vec3::ZERO).nodes))
        );

        let redone = store.redo(RevisionId(3));
        assert_eq!(
            redone.as_ref().map(|s| (s.revision, &s.nodes)),
            Ok((RevisionId(4), &GraphSnapshot::demo(5, Vec3::ZERO).nodes))
        );
        assert_eq!(store.redo(RevisionId(4)), Err(StoreError::NothingToRedo));
    }
}
