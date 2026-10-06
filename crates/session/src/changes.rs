use crate::ClipChange;
use capopen_engine::{Project, edit::EditOutcome};
use std::collections::HashMap;

pub(crate) fn changes(
    before: &Project,
    after: &Project,
    outcome: &EditOutcome,
) -> (Vec<String>, Vec<ClipChange>) {
    let old: HashMap<_, _> = before
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (&c.id, (&t.id, c))))
        .collect();
    let mut changed = Vec::new();
    let mut clips = Vec::new();
    for track in &after.tracks {
        for clip in &track.clips {
            let differs = old
                .get(&clip.id)
                .is_some_and(|(track_id, c)| *track_id != &track.id || *c != clip);
            if differs {
                changed.push(clip.id.clone());
            }
            if differs || outcome.created.contains(&clip.id) {
                clips.push(ClipChange {
                    track_id: track.id.clone(),
                    clip: clip.clone(),
                });
            }
        }
    }
    (changed, clips)
}
