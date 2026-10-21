//! How a `match`'s cases are emitted, upstream's `emitCases` and
//! `emitCaseCluster` (`ParserStmts.cpp`) decided over the cases' command
//! lists in pattern order.
//!
//! A list of case entries is cut into runs (`findClusterSize`): a run is led
//! by a case whose first command tests a literal with a spelling or an enum
//! case, and takes every following case whose first command is of the same
//! kind at the same path, plus one trailing case that always matches while
//! every member is decided by that test alone. A run of one is a case
//! tested alone. A longer run is a cluster: its leading value is read once,
//! and one arm per distinct key — the literal's spelling, or the case's
//! index, sorted — runs its members' residuals (the leading test dropped,
//! the tests that remain ahead of the bindings) as a list of its own,
//! whose concluding entry coverage marks again. The trailing complement, a
//! case that always matches or the members a concluding case's key closes,
//! is the cluster's else arm. Unreachable cases take part, as upstream's
//! entries do; the emission seals them.

use super::coverage::{CoverCase, CoverCommand};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{
    CaseCluster, ClusterGroup, CommandRef, EmittedArm, MatchEmission, ResidualCase,
};

impl Checker {
    /// Plan how the `match` whose cases are `cases` (unreachable ones
    /// included), over a subject of type `subject`, is emitted, the case at
    /// `concluding` closing the subject's covering space.
    pub(super) fn plan_match_emission(
        &self,
        subject: &Ty,
        cases: &[CoverCase],
        concluding: Option<usize>,
    ) -> Result<MatchEmission, TypeError> {
        let entries: Vec<Entry> = cases
            .iter()
            .enumerate()
            .map(|(case, cover)| Entry {
                case,
                commands: (0..cover.commands.len()).collect(),
                concluding: concluding == Some(case),
                cover,
            })
            .collect();
        self.emit_cases(subject, &entries)
    }

    /// One list of entries, upstream's `emitCases`: its runs, each a case
    /// alone or a cluster.
    fn emit_cases(&self, subject: &Ty, entries: &[Entry]) -> Result<MatchEmission, TypeError> {
        let mut arms = Vec::new();
        let mut rest = entries;
        while !rest.is_empty() {
            let (run, after) = rest.split_at(find_cluster_size(rest));
            arms.push(match run {
                [entry] => EmittedArm::Case(entry.arm()),
                _ => EmittedArm::Cluster(self.emit_case_cluster(subject, run)?),
            });
            rest = after;
        }
        Ok(MatchEmission {
            arms,
            concluding: entries.iter().any(|entry| entry.concluding),
        })
    }

    /// A run of two or more entries sharing a leading test, upstream's
    /// `emitCaseCluster`: the trailing complement split off, the tested
    /// members sorted stably by key and grouped by it, each group's
    /// residuals a list of their own.
    fn emit_case_cluster(&self, subject: &Ty, entries: &[Entry]) -> Result<CaseCluster, TypeError> {
        let last = entries.len() - 1;
        let (tested, complement, drop_test) = if entries[last].always_matches(false) {
            (&entries[..last], &entries[last..], false)
        } else if entries[last].concluding {
            // A concluding case's key closes the subject: the trailing
            // members with that key run as the else arm, their test
            // dropped, unless the key occurs earlier too.
            let key = entries[last].key();
            let mut start = last;
            while start != 0 && entries[start - 1].key() == key {
                start -= 1;
            }
            let earlier = entries[..start].iter().any(|entry| entry.key() == key);
            if !earlier && start != 0 {
                (&entries[..start], &entries[start..], true)
            } else {
                (entries, &entries[..0], false)
            }
        } else {
            (entries, &entries[..0], false)
        };
        let mut ordered: Vec<(Key, &Entry)> =
            tested.iter().map(|entry| (entry.key(), entry)).collect();
        ordered.sort_by(|(left, _), (right, _)| left.cmp(right));
        let mut groups = Vec::new();
        for group in ordered.chunk_by(|(left, _), (right, _)| left == right) {
            let residuals: Vec<Entry> = group
                .iter()
                .map(|(_, entry)| entry.residual_entry())
                .collect();
            let residuals = self.mark_residual_concluding(subject, residuals)?;
            groups.push(ClusterGroup {
                test: group[0].1.leading(),
                residuals: self.emit_cases(subject, &residuals)?,
            });
        }
        let complement = if complement.is_empty() {
            None
        } else {
            let entries = complement
                .iter()
                .map(|entry| {
                    if drop_test {
                        entry.residual_entry()
                    } else {
                        entry.clone()
                    }
                })
                .collect();
            let entries = self.mark_residual_concluding(subject, entries)?;
            Some(self.emit_cases(subject, &entries)?)
        };
        Ok(CaseCluster {
            tested: tested[0].leading(),
            groups,
            complement,
            concluding: entries.iter().any(|entry| entry.concluding),
        })
    }

    /// `entries` with the one that closes the subject's covering space over
    /// their residual commands marked concluding, upstream's
    /// `markResidualConcludingCase`.
    fn mark_residual_concluding<'c>(
        &self,
        subject: &Ty,
        mut entries: Vec<Entry<'c>>,
    ) -> Result<Vec<Entry<'c>>, TypeError> {
        let covers: Vec<CoverCase> = entries
            .iter()
            .map(|entry| CoverCase {
                commands: entry.commands().cloned().collect(),
                guarded: entry.cover.guarded,
                span: entry.cover.span.clone(),
            })
            .collect();
        let concluding = self.residual_concluding(subject, &covers)?;
        for (index, entry) in entries.iter_mut().enumerate() {
            entry.concluding = concluding == Some(index);
        }
        Ok(entries)
    }
}

/// One case entry of a list: the case, the commands it still runs (indices
/// into its pattern-order command list), and whether it concludes the list.
#[derive(Clone)]
struct Entry<'c> {
    case: usize,
    commands: Vec<usize>,
    concluding: bool,
    cover: &'c CoverCase,
}

impl Entry<'_> {
    /// The commands the entry still runs, in order.
    fn commands(&self) -> impl Iterator<Item = &CoverCommand> {
        self.commands
            .iter()
            .map(|&index| &self.cover.commands[index])
    }

    /// Whether the entry cannot fail once reached, upstream's
    /// `MatchCaseEntry::alwaysMatches`: unguarded, and every command (the
    /// first skipped when `skip_first`) a binding.
    fn always_matches(&self, skip_first: bool) -> bool {
        !self.cover.guarded
            && self
                .commands()
                .skip(usize::from(skip_first))
                .all(CoverCommand::is_bind)
    }

    /// The entry's first command, if any.
    fn first(&self) -> Option<&CoverCommand> {
        self.commands().next()
    }

    /// The sort key of the entry's leading test, upstream's
    /// `SpellingOrEnumCase::getLeadingTest`: an enum case's index, or a
    /// literal's spelling (empty when it has none).
    fn key(&self) -> Key {
        match self.first() {
            Some(CoverCommand::EnumTag { case, .. }) => Key {
                case: *case,
                spelling: String::new(),
            },
            Some(CoverCommand::Equal { spelling, .. }) => Key {
                case: 0,
                spelling: spelling.clone().unwrap_or_default(),
            },
            _ => Key::default(),
        }
    }

    /// The entry's leading command.
    fn leading(&self) -> CommandRef {
        CommandRef {
            case: self.case,
            command: self.commands[0],
        }
    }

    /// The entry as an arm runs it.
    fn arm(&self) -> ResidualCase {
        ResidualCase {
            case: self.case,
            commands: self.commands.clone(),
        }
    }

    /// The entry with its leading test handled, upstream's
    /// `internResidualCommandList`: the remaining tests, then the
    /// bindings, so that a further run can share the next test.
    fn residual_entry(&self) -> Self {
        let rest = &self.commands[1..];
        let binds = |bind: bool| {
            rest.iter()
                .copied()
                .filter(move |&index| self.cover.commands[index].is_bind() == bind)
        };
        Self {
            commands: binds(false).chain(binds(true)).collect(),
            concluding: false,
            ..self.clone()
        }
    }
}

/// A cluster's sort key, upstream's `SpellingOrEnumCase`: the enum case's
/// index first, then the literal's spelling, bytewise.
#[derive(Default, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    case: usize,
    spelling: String,
}

/// How many leading entries of `entries` form one run, upstream's
/// `findClusterSize`: one unless the first entry's first command tests a
/// literal with a spelling or an enum case; then every following entry
/// whose first command is of the same kind at the same path, and one
/// trailing entry that always matches while every member before it is
/// decided by its leading test alone.
fn find_cluster_size(entries: &[Entry]) -> usize {
    let Some(first) = entries[0].first() else {
        return 1;
    };
    let clusters = match first {
        CoverCommand::EnumTag { .. } => true,
        CoverCommand::Equal { spelling, .. } => spelling
            .as_deref()
            .is_some_and(|spelling| !spelling.is_empty()),
        _ => false,
    };
    if !clusters {
        return 1;
    }
    let mut size = 0;
    let mut exclusive = true;
    for entry in entries {
        if exclusive && entry.always_matches(false) {
            return size + 1;
        }
        let Some(next) = entry.first() else {
            break;
        };
        if std::mem::discriminant(next) != std::mem::discriminant(first)
            || next.path() != first.path()
        {
            break;
        }
        exclusive &= entry.always_matches(true);
        size += 1;
    }
    size
}

#[cfg(test)]
mod tests {
    use super::super::patterns::Step;
    use super::{CoverCase, CoverCommand, Entry, Key, find_cluster_size};
    use mojito_common::token::SourceSpan;

    fn equal(path: &[Step], spelling: Option<&str>) -> CoverCommand {
        CoverCommand::Equal {
            path: path.to_vec(),
            spelling: spelling.map(str::to_string),
        }
    }

    fn tag(case: usize) -> CoverCommand {
        CoverCommand::EnumTag {
            path: Vec::new(),
            case,
        }
    }

    fn bind(path: &[Step]) -> CoverCommand {
        CoverCommand::Bind {
            path: path.to_vec(),
        }
    }

    fn case(commands: Vec<CoverCommand>, guarded: bool) -> CoverCase {
        CoverCase {
            commands,
            guarded,
            span: SourceSpan::new(None, (0, 0)),
        }
    }

    fn entries(cases: &[CoverCase]) -> Vec<Entry<'_>> {
        cases
            .iter()
            .enumerate()
            .map(|(index, cover)| Entry {
                case: index,
                commands: (0..cover.commands.len()).collect(),
                concluding: false,
                cover,
            })
            .collect()
    }

    #[test]
    fn spelled_literals_on_one_path_form_one_run_with_a_trailing_wildcard() {
        let cases = [
            case(vec![equal(&[], Some("2"))], false),
            case(vec![equal(&[], Some("1"))], false),
            case(vec![equal(&[], Some("2"))], false),
            case(Vec::new(), false),
        ];
        assert_eq!(find_cluster_size(&entries(&cases)), 4);
    }

    #[test]
    fn a_guarded_member_keeps_the_trailing_wildcard_out_of_the_run() {
        let cases = [
            case(vec![equal(&[], Some("1"))], true),
            case(vec![equal(&[], Some("2"))], false),
            case(Vec::new(), false),
        ];
        assert_eq!(find_cluster_size(&entries(&cases)), 2);
    }

    #[test]
    fn a_leading_binding_or_unspelled_equality_is_a_run_of_one() {
        let element = [Step::Element(1)];
        let bound = [
            case(
                vec![bind(&[Step::Element(0)]), equal(&element, Some("3"))],
                false,
            ),
            case(
                vec![bind(&[Step::Element(0)]), equal(&element, Some("1"))],
                false,
            ),
        ];
        assert_eq!(find_cluster_size(&entries(&bound)), 1);
        let unspelled = [
            case(vec![equal(&[], None)], false),
            case(vec![equal(&[], Some("1"))], false),
        ];
        assert_eq!(find_cluster_size(&entries(&unspelled)), 1);
        // A wildcard element has no command, so the next test leads.
        let wildcard = [
            case(vec![equal(&element, Some("3"))], false),
            case(vec![equal(&element, Some("1"))], false),
            case(vec![equal(&[Step::Element(0)], Some("1"))], false),
        ];
        assert_eq!(find_cluster_size(&entries(&wildcard)), 2);
    }

    #[test]
    fn keys_sort_by_case_index_then_spelling_bytewise() {
        let cases = [
            case(vec![equal(&[], Some("4"))], false),
            case(vec![equal(&[], Some("0x4"))], false),
            case(vec![tag(1)], false),
            case(vec![tag(0)], false),
        ];
        let keys: Vec<Key> = entries(&cases).iter().map(Entry::key).collect();
        assert!(keys[1] < keys[0]);
        assert!(keys[3] < keys[2]);
    }

    #[test]
    fn a_residual_runs_its_tests_before_its_bindings() {
        let payload = [Step::EnumPayload(1)];
        let cases = [case(
            vec![
                tag(1),
                bind(&[Step::Element(0)]),
                equal(&payload, Some("2")),
            ],
            false,
        )];
        let residual = entries(&cases)[0].residual_entry();
        assert_eq!(residual.commands, vec![2, 1]);
    }
}
