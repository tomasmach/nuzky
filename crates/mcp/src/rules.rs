//! Access rules of every tool, in one table.
use serde_json::Value;

#[derive(Clone, Copy)]
pub(crate) enum Reads {
    Always,
    Never,
    /// Only with these arguments, such as job(get) or a dry run.
    When(fn(&Value) -> bool),
}

pub(crate) struct Rules {
    pub name: &'static str,
    /// Reads take no run lock and are open to read-only clients.
    pub reads: Reads,
    pub destructive: bool,
    pub idempotent: bool,
    /// The job joins the caller's open run, so stopping that run cancels it.
    pub run_job: bool,
}

const fn rules(name: &'static str, reads: Reads, destructive: bool, idempotent: bool, run_job: bool) -> Rules {
    Rules { name, reads, destructive, idempotent, run_job }
}

use Reads::{Always, Never, When};

pub(crate) const TOOLS: &[Rules] = &[
    rules("get_state", Always, false, true, false),
    rules("begin_run", Never, false, false, false),
    rules("resolve_recovery", Never, true, false, false),
    rules("apply_edits", Never, true, true, false),
    rules("end_run", Never, true, false, false),
    rules("undo_run", Never, true, false, false),
    rules("import_media", Never, false, false, false),
    rules("inspect_frames", Always, false, true, false),
    // Retakes and emphasis only read stored words and sound and answer at once.
    rules("analyze", When(|args| args["kind"] == "retakes" || args["kind"] == "emphasis"), false, false, true),
    rules("transcribe", Never, false, false, true),
    rules("get_transcript", Always, false, true, false),
    rules("edit_transcript", When(|args| args["dry_run"] == true), true, false, false),
    rules("job", When(|args| args["action"] == "get"), false, false, false),
    rules("build_captions", Never, true, false, false),
    rules("apply_zooms", Never, true, false, false),
    rules("export_video", Never, false, false, true),
];

pub(crate) fn find(name: &str) -> Option<&'static Rules> {
    TOOLS.iter().find(|rules| rules.name == name)
}

impl Rules {
    pub fn reads(&self, arguments: &Value) -> bool {
        match self.reads {
            Always => true,
            Never => false,
            When(reads) => reads(arguments),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn every_tool_is_classified_once_and_rules_are_unchanged() {
        let catalog: BTreeSet<_> = crate::catalog().unwrap().into_iter().map(|tool| tool.name.to_string()).collect();
        let table: BTreeSet<_> = TOOLS.iter().map(|rules| rules.name.to_string()).collect();
        assert_eq!(catalog, table);
        assert_eq!(table.len(), TOOLS.len(), "a tool is listed twice");
        let names =
            |keep: fn(&Rules) -> bool| TOOLS.iter().filter(|r| keep(r)).map(|r| r.name).collect::<BTreeSet<_>>();
        let set = |names: &[&'static str]| names.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(names(|r| matches!(r.reads, Always)), set(&["get_state", "get_transcript", "inspect_frames"]));
        assert_eq!(
            names(|r| r.destructive),
            set(&[
                "apply_edits",
                "edit_transcript",
                "end_run",
                "undo_run",
                "build_captions",
                "apply_zooms",
                "resolve_recovery"
            ])
        );
        assert_eq!(names(|r| r.idempotent), set(&["get_state", "get_transcript", "inspect_frames", "apply_edits"]));
        assert_eq!(names(|r| r.run_job), set(&["analyze", "transcribe", "export_video"]));
        let job = find("job").unwrap();
        assert!(job.reads(&json!({"action":"get"})) && !job.reads(&json!({"action":"cancel"})));
        let edit = find("edit_transcript").unwrap();
        assert!(edit.reads(&json!({"dry_run":true})) && !edit.reads(&json!({})));
        let analyze = find("analyze").unwrap();
        assert!(analyze.reads(&json!({"kind":"retakes"})) && !analyze.reads(&json!({"kind":"silences"})));
        assert!(analyze.reads(&json!({"kind":"emphasis"})));
        assert!(find("unknown").is_none());
    }
}
