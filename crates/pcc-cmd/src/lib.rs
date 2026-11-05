//! Revision-checked command boundary.
use pcc_graph::{GraphEdit, GraphSnapshot, SceneError};
use pcc_id::RevisionId;
#[derive(Debug, Eq, PartialEq)]
pub enum CommandError {
    Conflict {
        expected: RevisionId,
        actual: RevisionId,
    },
    Scene(SceneError),
}
pub enum EngineCommand<'a> {
    ReplaceScene {
        base: RevisionId,
        ofxs: &'a str,
    },
    BatchEdit {
        base: RevisionId,
        edits: &'a [GraphEdit],
    },
}
pub fn publish(
    current: &GraphSnapshot,
    base: RevisionId,
    mut next: GraphSnapshot,
) -> Result<GraphSnapshot, CommandError> {
    if current.revision != base {
        return Err(CommandError::Conflict {
            expected: base,
            actual: current.revision,
        });
    }
    next.revision = current.revision.next();
    Ok(next)
}
pub fn execute(
    current: &GraphSnapshot,
    command: EngineCommand<'_>,
) -> Result<GraphSnapshot, CommandError> {
    match command {
        EngineCommand::ReplaceScene { base, ofxs } => {
            let scene = GraphSnapshot::from_ofxs(ofxs).map_err(CommandError::Scene)?;
            publish(current, base, scene)
        }
        EngineCommand::BatchEdit { base, edits } => {
            if current.revision != base {
                return Err(CommandError::Conflict {
                    expected: base,
                    actual: current.revision,
                });
            }
            current
                .apply_edits(current.revision.next(), edits)
                .map_err(CommandError::Scene)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcc_id::NodeId;
    use pcc_math::Vec3;
    #[test]
    fn commands_require_the_expected_revision() {
        let current = GraphSnapshot::demo(1, Vec3::ZERO);
        let result = execute(
            &current,
            EngineCommand::ReplaceScene {
                base: RevisionId(0),
                ofxs: &current.to_ofxs(),
            },
        );
        assert!(matches!(result, Err(CommandError::Conflict { .. })));

        let batch = execute(
            &current,
            EngineCommand::BatchEdit {
                base: RevisionId(1),
                edits: &[GraphEdit::SetParam {
                    id: NodeId(1),
                    name: "count".to_owned(),
                    value: pcc_graph::ParamValue::Uint(32),
                }],
            },
        );
        assert_eq!(batch.map(|g| g.revision), Ok(RevisionId(2)));
    }
}
