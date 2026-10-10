//! EDIT.md in blocks. A block starts at a heading Nuzky writes and runs to the next one, so one
//! rule can be added, replaced or removed while everything else stays as the creator left it.
//! Text before the first such heading is the preamble; other headings, such as the creator's own
//! sections or a rule's subsections, belong to the block above them. A section of the creator's
//! own stays when the block above it is written again or removed.

use std::collections::BTreeSet;

use super::{RULES, SETTINGS, settings_block};

pub const OWN: &str = "## Your rules";
pub const SETTINGS_HEADING: &str = "## Settings";
pub const OVERVIEW: &str = "## What gets cut";
pub const RARE: &str = "## Seen too rarely for a rule";
/// How a style with only the creator's own rules starts.
pub const PREAMBLE: &str = "# Editing style\n\nAn agent editing through Nuzky follows the rules here instead of the general defaults in nuzky://guide, and keeps the defaults for anything this file does not cover.\n\n";
/// How the preamble of a learned style starts.
const LEARNED: &str = "# Editing style\n\nNuzky measured how these recordings became their finished cuts.";
const OWN_INTRO: &str = "The creator's own instructions. They win over every rule below and over the guide.";
/// Subheadings a rule writes itself.
const SUBHEADINGS: &[&str] = &["### Zoom ins", "### Zoom outs", "### Slow moves"];

/// Where the creator's own sections start in a block: the first heading after its first line
/// that Nuzky does not write.
fn foreign(text: &str) -> usize {
    let mut at = 0;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let bare = line.trim_end();
        if i > 0 && bare.starts_with('#') && !SUBHEADINGS.contains(&bare) {
            return at;
        }
        at += line.len();
    }
    text.len()
}

/// Every heading that starts a block, in the order blocks are written.
fn headings() -> impl Iterator<Item = &'static str> {
    [OWN, SETTINGS_HEADING, OVERVIEW].into_iter().chain(RULES.iter().copied()).chain([RARE])
}

/// Where a block goes: the preamble first, then in the order of `headings`.
fn rank(heading: Option<&str>) -> usize {
    heading.map_or(0, |h| 1 + headings().position(|known| known == h).unwrap_or(usize::MAX - 1))
}

/// The heading of the rule named `title`.
pub fn rule_heading(title: &str) -> Option<&'static str> {
    RULES.iter().copied().find(|h| h.trim_start_matches('#').trim_start() == title)
}

/// The name of a rule from its heading.
pub fn title(heading: &str) -> &str {
    heading.trim_start_matches('#').trim_start()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// None for the preamble.
    pub heading: Option<&'static str>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    /// The preamble first, then the rest in file order.
    pub blocks: Vec<Block>,
}

impl Doc {
    pub fn parse(text: &str) -> Self {
        let mut blocks = vec![Block { heading: None, text: String::new() }];
        for line in text.split_inclusive('\n') {
            let bare = line.trim_end_matches(['\n', '\r']).trim_end();
            match headings().find(|h| *h == bare) {
                Some(heading) if !blocks.iter().any(|b| b.heading == Some(heading)) => {
                    blocks.push(Block { heading: Some(heading), text: line.to_owned() });
                }
                _ => blocks.last_mut().expect("the preamble is always there").text.push_str(line),
            }
        }
        Self { blocks }
    }

    pub fn text(&self) -> String {
        self.blocks.iter().map(|b| b.text.as_str()).collect()
    }

    pub fn get(&self, heading: &str) -> Option<&str> {
        self.blocks.iter().find(|b| b.heading == Some(heading)).map(|b| b.text.as_str())
    }

    pub fn preamble(&self) -> &str {
        &self.blocks[0].text
    }

    /// The preamble is one Nuzky wrote, so learning may write it again.
    pub fn preamble_is_ours(&self) -> bool {
        let preamble = self.preamble();
        preamble.trim().is_empty() || preamble == PREAMBLE || preamble.starts_with(LEARNED)
    }

    /// Writes the preamble again, keeping the creator's own sections in it.
    pub fn set_preamble(&mut self, text: String) {
        let old = &self.blocks[0].text;
        self.blocks[0].text = text + &old[foreign(old)..];
    }

    /// Writes the block again, keeping the creator's own sections in it, or puts it where its
    /// heading belongs.
    pub fn set(&mut self, heading: &'static str, mut text: String) {
        if let Some(block) = self.blocks.iter_mut().find(|b| b.heading == Some(heading)) {
            // The creator's sections stay as they are here, never doubled by ones the new text brings.
            text.truncate(foreign(&text));
            block.text = text + &block.text[foreign(&block.text)..];
            return;
        }
        let at = self.blocks.iter().rposition(|b| rank(b.heading) <= rank(Some(heading))).map_or(0, |i| i + 1);
        self.blocks.insert(at, Block { heading: Some(heading), text });
    }

    /// Removes the block; the creator's own sections in it join the block above.
    pub fn remove(&mut self, heading: &str) {
        let Some(at) = self.blocks.iter().position(|b| b.heading == Some(heading)) else { return };
        let block = self.blocks.remove(at);
        self.blocks[at - 1].text.push_str(&block.text[foreign(&block.text)..]);
    }

    /// The rules in the file, by title, in file order.
    pub fn rules(&self) -> Vec<&'static str> {
        self.blocks.iter().filter_map(|b| b.heading.filter(|h| RULES.contains(h)).map(title)).collect()
    }

    /// The rows of the Settings table.
    pub fn settings(&self) -> Vec<(String, String)> {
        let Some(block) = self.get(SETTINGS_HEADING) else { return Vec::new() };
        block
            .lines()
            .filter(|l| l.trim_start().starts_with('|'))
            .skip(2)
            .filter_map(|l| {
                let cells: Vec<&str> = l.trim().trim_start_matches('|').trim_end_matches('|').split('|').collect();
                match cells[..] {
                    [name, value] => Some((name.trim().to_owned(), value.trim().to_owned())),
                    _ => None,
                }
            })
            .collect()
    }

    /// Writes the Settings table again: each rule's rows in rule order, `rows` for `title`, then
    /// rows no rule writes. Other text in the Settings section stays; without rows or other text
    /// there is no section.
    pub fn rebuild_settings(&mut self, title: Option<(&str, &[(String, String)])>) {
        let old = self.settings();
        let owner = |name: &str| SETTINGS.iter().find(|(n, _)| *n == name).map(|(_, owner)| *owner);
        let mut rows: Vec<(String, String)> = Vec::new();
        for rule in self.rules() {
            match title {
                Some((t, new)) if t == rule => rows.extend(new.iter().cloned()),
                _ => rows.extend(old.iter().filter(|(name, _)| owner(name) == Some(rule)).cloned()),
            }
        }
        rows.extend(old.iter().filter(|(name, _)| owner(name).is_none()).cloned());
        let Some(block) = self.blocks.iter_mut().find(|b| b.heading == Some(SETTINGS_HEADING)) else {
            if !rows.is_empty() {
                self.set(SETTINGS_HEADING, settings_block(&rows));
            }
            return;
        };
        let table = settings_block(&rows);
        let table = if rows.is_empty() { "" } else { &table[SETTINGS_HEADING.len() + 2..table.len() - 1] };
        let lines: Vec<&str> = block.text.split_inclusive('\n').collect();
        let is_row = |l: &str| l.trim_start().starts_with('|');
        let text = match lines.iter().position(|l| is_row(l)) {
            Some(first) => {
                let end = first + lines[first..].iter().take_while(|l| is_row(l)).count();
                [lines[..first].concat(), table.to_owned(), lines[end..].concat()].concat()
            }
            None => [lines[..1].concat(), "\n".to_owned(), table.to_owned(), lines[1..].concat()].concat(),
        };
        if rows.is_empty() && text.lines().skip(1).all(|l| l.trim().is_empty()) {
            self.remove(SETTINGS_HEADING);
        } else {
            block.text = text;
        }
    }

    /// The creator's own rules: the list items of their block.
    pub fn own(&self) -> Vec<String> {
        self.get(OWN)
            .map(|block| block.lines().filter_map(|l| l.strip_prefix("- ")).map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Adds, changes (`Some(i)`, text) or removes (`Some(i)`, none) one of the creator's rules,
    /// leaving any other text in their block as it is.
    pub fn set_own(&mut self, index: Option<usize>, text: Option<&str>) {
        let block = self.get(OWN).map_or_else(|| format!("{OWN}\n\n{OWN_INTRO}\n\n\n"), str::to_owned);
        let mut lines: Vec<String> = block.split_inclusive('\n').map(str::to_owned).collect();
        let items: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].starts_with("- ")).collect();
        let line = text.map(|t| format!("- {t}\n"));
        match (index.and_then(|i| items.get(i).copied()), line) {
            (Some(at), Some(line)) => lines[at] = line,
            (Some(at), None) => drop(lines.remove(at)),
            (None, Some(line)) => {
                // After the last item, else before the blank line that ends the block.
                let at = items.last().map_or_else(|| lines.len().saturating_sub(1), |&i| i + 1);
                lines.insert(at, line);
            }
            (None, None) => {}
        }
        let block: String = lines.concat();
        if !block.lines().any(|l| l.starts_with("- ")) && block.trim() == format!("{OWN}\n\n{OWN_INTRO}") {
            self.remove(OWN);
        } else {
            self.set(OWN, block);
        }
        if self.preamble().trim().is_empty() {
            self.set_preamble(PREAMBLE.to_owned());
        }
    }

    /// Nothing an agent could follow: no rules of either kind and only Nuzky's own preamble.
    pub fn is_empty(&self) -> bool {
        let preamble = self.preamble().trim();
        self.blocks.len() == 1 && (preamble.is_empty() || preamble == PREAMBLE.trim())
    }
}

/// What the creator changed between two texts of the style, as the names learning must leave
/// alone: rules whose section or settings changed, "header", "overview" or "rare".
pub fn edited(old: &Doc, new: &Doc) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if old.preamble() != new.preamble() {
        out.insert("header".to_owned());
    }
    for (heading, name) in [(OVERVIEW, "overview"), (RARE, "rare")] {
        if old.get(heading) != new.get(heading) {
            out.insert(name.to_owned());
        }
    }
    for heading in RULES {
        if old.get(heading) != new.get(heading) {
            out.insert(title(heading).to_owned());
        }
    }
    let rows = |doc: &Doc, owner: &str| -> Vec<(String, String)> {
        let names: Vec<&str> = SETTINGS.iter().filter(|(_, o)| *o == owner).map(|(n, _)| *n).collect();
        doc.settings().into_iter().filter(|(n, _)| names.contains(&n.as_str())).collect()
    };
    for (_, owner) in SETTINGS {
        if rows(old, owner) != rows(new, owner) {
            out.insert((*owner).to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: &str = "# Editing style\n\nIntro.\n\n## Settings\n\n| Setting | Value |\n|---|---|\n| edit_transcript shorten_pauses_us | 420000 |\n| Cuts per minute | 6.0 |\n\n## What gets cut\n\nKept 80%.\n\n### Restarted sentences\n\nCut them.\n\n- 0:01.0 cut \"a\"\n\n## My notes\n\nMine.\n\n## Pauses\n\nShorten.\n\n## Cuts\n\nSix.\n\n## Zoom\n\nBase.\n\n### Zoom ins\n\nIns.\n\n";

    #[test]
    fn blocks_give_back_the_text_and_keep_foreign_headings_with_the_block_above() {
        let doc = Doc::parse(STYLE);
        assert_eq!(doc.text(), STYLE);
        assert_eq!(doc.rules(), ["Restarted sentences", "Pauses", "Cuts", "Zoom"]);
        assert!(doc.get("### Restarted sentences").unwrap().ends_with("## My notes\n\nMine.\n\n"));
        assert!(doc.get("## Zoom").unwrap().contains("### Zoom ins"));
        assert_eq!(doc.settings()[1], ("Cuts per minute".to_owned(), "6.0".to_owned()));
    }

    #[test]
    fn a_rule_goes_in_its_place_and_takes_its_settings_with_it() {
        let mut doc = Doc::parse(STYLE);
        doc.set("### Filler words", "### Filler words\n\nCut um.\n\n".into());
        doc.rebuild_settings(Some(("Filler words", &[("Filler words to cut".into(), "um".into())])));
        let text = doc.text();
        assert!(text.find("### Filler words").unwrap() < text.find("## Pauses").unwrap(), "{text}");
        assert!(text.find("### Restarted").unwrap() < text.find("### Filler words").unwrap(), "{text}");
        assert!(text.contains("|---|---|\n| Filler words to cut | um |\n| edit_transcript"), "{text}");
        doc.remove("## Pauses");
        doc.rebuild_settings(None);
        assert!(!doc.text().contains("shorten_pauses_us") && doc.text().contains("| Cuts per minute | 6.0 |"));
        doc.remove("## Cuts");
        doc.remove("### Filler words");
        doc.rebuild_settings(None);
        assert!(!doc.text().contains("## Settings"), "{}", doc.text());
    }

    #[test]
    fn a_learned_preamble_is_nuzkys_and_a_hand_written_one_is_not() {
        use crate::style::{Learned, Rule};
        let learned = Learned {
            header: crate::style::learned(&[]).header,
            overview: String::new(),
            rules: Vec::<Rule>::new(),
            rare: None,
        };
        assert!(Doc::parse(&learned.document()).preamble_is_ours());
        assert!(Doc::parse(PREAMBLE).preamble_is_ours());
        assert!(!Doc::parse("Cut hard, keep it short.\n").preamble_is_ours());
    }

    #[test]
    fn own_rules_are_list_items_and_an_empty_style_is_empty() {
        let mut doc = Doc::parse("");
        assert!(doc.is_empty());
        doc.set_own(None, Some("Never cut the product name."));
        doc.set_own(None, Some("Keep my sign-off."));
        assert_eq!(doc.own(), ["Never cut the product name.", "Keep my sign-off."]);
        assert!(doc.text().starts_with(&format!("{PREAMBLE}{OWN}\n\n{OWN_INTRO}\n\n- Never cut")), "{}", doc.text());
        assert!(doc.text().ends_with("- Keep my sign-off.\n\n"), "{:?}", doc.text());
        doc.set_own(Some(0), Some("Never cut the name."));
        doc.set_own(Some(1), None);
        assert_eq!(doc.own(), ["Never cut the name."]);
        doc.set_own(Some(0), None);
        assert!(doc.is_empty(), "{:?}", doc.text());

        let mut doc = Doc::parse(STYLE);
        doc.set_own(None, Some("Short."));
        let text = doc.text();
        assert!(
            text.find(OWN).unwrap() < text.find("## Settings").unwrap()
                && text.starts_with("# Editing style\n\nIntro.")
        );
    }

    #[test]
    fn the_creators_own_sections_outlive_rules_written_again_or_removed() {
        let mut doc = Doc::parse(&STYLE.replace(
            "| Cuts per minute | 6.0 |\n",
            "| Cuts per minute | 6.0 |\n\nKeep the brand name.\n\n## Brand\n\nAlways say Nuzky.\n",
        ));
        doc.set("### Restarted sentences", "### Restarted sentences\n\nCut them all.\n\n".into());
        doc.set("## Zoom", "## Zoom\n\nNone.\n\n".into());
        doc.rebuild_settings(Some(("Cuts", &[("Cuts per minute".into(), "7.0".into())])));
        let text = doc.text();
        assert!(text.contains("### Restarted sentences\n\nCut them all.\n\n## My notes\n\nMine.\n\n"), "{text}");
        assert!(text.contains("## Zoom\n\nNone.\n\n") && !text.contains("### Zoom ins"), "{text}");
        assert!(
            text.contains("| Cuts per minute | 7.0 |\n\nKeep the brand name.\n\n## Brand\n\nAlways say Nuzky.\n"),
            "{text}"
        );
        doc.remove("### Restarted sentences");
        doc.remove("## Pauses");
        doc.remove("## Cuts");
        doc.rebuild_settings(None);
        let text = doc.text();
        assert!(text.contains("Kept 80%.\n\n## My notes\n\nMine.\n\n## Zoom"), "{text}");
        assert!(
            text.contains("## Settings\n\n\nKeep the brand name.\n\n## Brand") && !text.contains("| Setting"),
            "{text}"
        );
    }

    #[test]
    fn the_creators_sections_under_their_rules_stay_once() {
        let mut doc = Doc::parse(&format!("{PREAMBLE}{OWN}\n\n{OWN_INTRO}\n\n- One.\n\n## Brand\n\nSay Nuzky.\n\n"));
        doc.set_own(Some(0), Some("Uno."));
        doc.set_own(None, Some("Two."));
        let text = doc.text();
        assert_eq!(text.matches("## Brand").count(), 1, "{text}");
        assert_eq!(doc.own(), ["Uno.", "Two."]);
        // A block put back from another version brings its own sections only once, too.
        let block = doc.get(OWN).unwrap().to_owned();
        doc.set(OWN, block);
        assert_eq!(doc.text(), text);
    }

    #[test]
    fn edits_name_the_rules_they_touch() {
        let old = Doc::parse(STYLE);
        let new = Doc::parse(
            &STYLE.replace("| 420000 |", "| 300000 |").replace("Base.", "Base 1.2.").replace("Mine.", "Ours."),
        );
        assert_eq!(edited(&old, &new).into_iter().collect::<Vec<_>>(), ["Pauses", "Restarted sentences", "Zoom"]);
        assert!(edited(&old, &old).is_empty());
    }
}
