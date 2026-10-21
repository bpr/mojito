//! `match` coverage, upstream's `checkCaseExhaustivityAndUnreachability`:
//! each case's commands, recorded as their shapes at their paths from the
//! subject, are credited in case order against one covering space for the
//! subject's type. The space proves when the cases cover every value, finds
//! the cases earlier ones already cover, and names the values none covers.
//!
//! The space is a closed `EnumLike`'s constructors (the modelled `Bool`'s
//! `False` and `True` among them), an open one's (`_enum_is_exhaustive =
//! False`, which only a catch-all closes), the Cartesian cells of a tuple or
//! struct whose every leaf is a closed `EnumLike` (at most 1024, else too
//! complex to track), or, over any other type, the spellings of the root
//! literals compared so far, which find duplicates and prove nothing. A
//! constructor or cell is covered only by a pattern that matches all of it:
//! a payload test refines without covering, a guard adds nothing, and `|`
//! credits each alternative.

use super::patterns::Step;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::Warning;

/// One command of a case's pattern as coverage reads it, at its path from
/// the match subject: an equality test with its literal operand's spelling,
/// an enum case's tag test, a sequence length test, a binding, or a `|`
/// pattern's alternatives.
#[derive(Clone)]
pub(super) enum CoverCommand {
    Equal {
        path: Vec<Step>,
        spelling: Option<String>,
    },
    EnumTag {
        path: Vec<Step>,
        case: usize,
    },
    ListLength {
        path: Vec<Step>,
    },
    Bind {
        path: Vec<Step>,
    },
    Or(Vec<Vec<Self>>),
}

impl CoverCommand {
    /// The path the command tests or binds; a `|` sits at the root.
    pub(super) fn path(&self) -> &[Step] {
        match self {
            Self::Equal { path, .. }
            | Self::EnumTag { path, .. }
            | Self::ListLength { path }
            | Self::Bind { path } => path,
            Self::Or(_) => &[],
        }
    }

    pub(super) const fn is_bind(&self) -> bool {
        matches!(self, Self::Bind { .. })
    }
}

/// One case as coverage reads it: its pattern's commands, whether a guard
/// follows them, and the pattern's span, where its warning goes.
#[derive(Clone)]
pub(super) struct CoverCase {
    pub(super) commands: Vec<CoverCommand>,
    pub(super) guarded: bool,
    pub(super) span: SourceSpan,
}

impl CoverCase {
    /// Whether the case cannot fail once reached: unguarded, and every
    /// command a binding (`_` has none).
    pub(super) fn always_matches(&self) -> bool {
        !self.guarded && self.commands.iter().all(CoverCommand::is_bind)
    }
}

/// What coverage proved of a `match`'s cases: the cases earlier ones
/// already cover, the case that covers the last open value (nothing falls
/// through past it), and the warnings in upstream's order — each
/// unreachable case's in case order, then the match's own.
pub(super) struct CoverageVerdict {
    pub(super) unreachable: Vec<bool>,
    pub(super) concluding: Option<usize>,
    pub(super) warnings: Vec<Warning>,
}

impl Checker {
    /// Credit `cases` in order against the covering space of the subject
    /// type `subject`, the match-level warning going at `span`.
    pub(super) fn match_coverage(
        &self,
        subject: &Ty,
        cases: &[CoverCase],
        span: &SourceSpan,
    ) -> Result<CoverageVerdict, TypeError> {
        let mut space = self.covering_space(subject)?;
        let mut unreachable = vec![false; cases.len()];
        let mut concluding = None;
        let mut warnings = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            if space.covered() {
                unreachable[index] = true;
                warnings.push(warning(UNREACHABLE.to_string(), &case.span));
                continue;
            }
            if case.always_matches() {
                space = Space::Closed;
                concluding = Some(index);
                continue;
            }
            if case.guarded {
                continue;
            }
            let Some(credit) = space.credit(&case.commands) else {
                continue;
            };
            if !credit.hit && credit.interpreted_all {
                unreachable[index] = true;
                let message = space.covered_value(&case.commands).map_or_else(
                    || UNREACHABLE.to_string(),
                    |value| {
                        format!(
                            "case is unreachable; '{value}' is already covered by a previous case"
                        )
                    },
                );
                warnings.push(warning(message, &case.span));
            } else if credit.hit && space.covered() {
                concluding = Some(index);
            }
        }
        warnings.extend(space.match_warning().map(|message| warning(message, span)));
        Ok(CoverageVerdict {
            unreachable,
            concluding,
            warnings,
        })
    }

    /// The case of the residual list `cases` that closes the covering
    /// space of the subject type `subject`, upstream's
    /// `markResidualConcludingCase`: the first that always matches, or the
    /// first unguarded one whose commands cover the last open value. The
    /// space is the whole subject's, so a residual whose tests sit below
    /// the root credits nothing. No warning is made.
    pub(super) fn residual_concluding(
        &self,
        subject: &Ty,
        cases: &[CoverCase],
    ) -> Result<Option<usize>, TypeError> {
        let mut space = self.covering_space(subject)?;
        for (index, case) in cases.iter().enumerate() {
            if case.always_matches() {
                return Ok(Some(index));
            }
            if case.guarded {
                continue;
            }
            if space
                .credit(&case.commands)
                .is_some_and(|credit| credit.hit && space.covered())
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    /// The covering space of the subject type `ty`, upstream's
    /// `createRootCoveringSpace`: an `EnumLike`'s constructors (opaque when
    /// its names are unknown or empty), else the cells of a product of
    /// closed `EnumLike` leaves, else the open set of literal spellings.
    fn covering_space(&self, ty: &Ty) -> Result<Space, TypeError> {
        if let Some(constructors) = self.enum_constructors(ty)? {
            let Some(names) = constructors.names.filter(|names| !names.is_empty()) else {
                return Ok(Space::Opaque);
            };
            return Ok(Space::Ctors {
                remaining: vec![true; names.len()],
                names,
                exhaustive: constructors
                    .exhaustive
                    .ok_or_else(exhaustiveness_not_static)?,
                universe_covered: false,
            });
        }
        let mut dims = Vec::new();
        if !self.finite_dims(Vec::new(), ty, &mut dims)? || dims.is_empty() {
            return Ok(Space::Literals(Vec::new()));
        }
        let ctors: Vec<usize> = dims.iter().map(|dim| dim.names.len()).collect();
        Ok(
            product_cells(&ctors).map_or(Space::TooComplex, |cells| Space::Product {
                dims,
                cells: vec![true; cells],
            }),
        )
    }

    /// Append the closed `EnumLike` leaves of `ty`, at `path`, to `dims`,
    /// upstream's `collectFiniteDims`: an `EnumLike` is one leaf, a tuple
    /// its elements' leaves, a struct its stored fields' in declaration
    /// order. Whether every leaf is a closed `EnumLike`; an open one, or
    /// any other type, makes the product the open literal set.
    fn finite_dims(
        &self,
        path: Vec<Step>,
        ty: &Ty,
        dims: &mut Vec<Dim>,
    ) -> Result<bool, TypeError> {
        if let Some(constructors) = self.enum_constructors(ty)? {
            let exhaustive = constructors
                .exhaustive
                .ok_or_else(exhaustiveness_not_static)?;
            let Some(names) = constructors
                .names
                .filter(|names| exhaustive && !names.is_empty())
            else {
                return Ok(false);
            };
            dims.push(Dim { path, names });
            return Ok(true);
        }
        let child = |step: Step| {
            let mut child = path.clone();
            child.push(step);
            child
        };
        if let Some(elements) = tuple_elements(ty.kind()) {
            for (index, element) in elements.into_iter().enumerate() {
                if !self.finite_dims(child(Step::Element(index)), element, dims)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return Ok(false);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(false);
        };
        for (field, field_ty) in &info.fields {
            let field_ty = substitute_at(field_ty, info, arguments);
            if !self.finite_dims(child(Step::Field(field.clone())), &field_ty, dims)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The constructors of `ty` when it is an `EnumLike`: the case names
    /// `_enum_case_names` folds to and the `_enum_is_exhaustive` flag, each
    /// unknown when it does not fold. The modelled `Bool` is upstream's
    /// library conformer (`bool.mojo`), its cases `False` and `True`.
    fn enum_constructors(&self, ty: &Ty) -> Result<Option<Constructors>, TypeError> {
        if matches!(ty.kind(), TyKind::Bool) {
            return Ok(Some(Constructors {
                names: Some(vec!["False".to_string(), "True".to_string()]),
                exhaustive: Some(true),
            }));
        }
        if !self.conforms_to(ty, &enum_like_trait()) {
            return Ok(None);
        }
        let names = self.enum_like_list(ty, "_enum_case_names")?.map(|names| {
            names
                .iter()
                .map(|name| match name.as_constant() {
                    Some(CtValue::KgenString(name) | CtValue::Str(name)) => name.clone(),
                    _ => "?".to_string(),
                })
                .collect()
        });
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return Ok(None);
        };
        let flag = match self.structs.get(name) {
            Some(info) => self
                .struct_value_member(info, arguments, "_enum_is_exhaustive")
                .transpose()?,
            None => None,
        };
        let exhaustive = flag.and_then(|flag| match flag.as_constant() {
            Some(CtValue::Bool(exhaustive)) => Some(*exhaustive),
            _ => None,
        });
        Ok(Some(Constructors { names, exhaustive }))
    }
}

/// The warning of a case every value of whose earlier cases cover.
const UNREACHABLE: &str =
    "case is unreachable; previous cases cover every value of the match subject";

/// The largest product of constructors the space tracks cell by cell,
/// upstream's `kMaxProductCells`.
const MAX_CELLS: usize = 1024;

/// How many uncovered values the missing-case warning names, upstream's
/// `kMaxSuggestedCases`.
const MAX_SUGGESTED: usize = 8;

/// The values a `match`'s cases have not yet covered, as upstream's
/// `MatchCoveringSpace` tracks them.
enum Space {
    /// An `EnumLike` whose case names are unknown or empty: only a
    /// catch-all closes it.
    Opaque,
    /// An `EnumLike` root: per constructor, whether it is still open. An
    /// open enum (not `exhaustive`) is closed by a catch-all alone, which
    /// covers its unknown values.
    Ctors {
        names: Vec<String>,
        remaining: Vec<bool>,
        exhaustive: bool,
        universe_covered: bool,
    },
    /// A tuple or struct of closed `EnumLike` leaves: per cell, row-major
    /// with the last leaf varying fastest, whether it is still open.
    Product { dims: Vec<Dim>, cells: Vec<bool> },
    /// A product above `MAX_CELLS`, tracked not at all.
    TooComplex,
    /// Any other subject: the root literal spellings compared so far.
    Literals(Vec<String>),
    /// Closed by a catch-all.
    Closed,
}

impl Space {
    /// Whether no value is left for a later case to match.
    fn covered(&self) -> bool {
        match self {
            Self::Closed => true,
            Self::Ctors {
                remaining,
                exhaustive,
                universe_covered,
                ..
            } => {
                if *exhaustive {
                    !remaining.contains(&true)
                } else {
                    *universe_covered
                }
            }
            Self::Product { cells, .. } => !cells.contains(&true),
            Self::Opaque | Self::TooComplex | Self::Literals(_) => false,
        }
    }

    /// Credit `commands`, each `|` alternative on its own, upstream's
    /// `coverCommands`: `None` when no alternative could be interpreted.
    fn credit(&mut self, commands: &[CoverCommand]) -> Option<Credit> {
        let mut alternatives = Vec::new();
        expand_or(commands.iter().collect(), &mut alternatives);
        let mut credit: Option<Credit> = None;
        let mut interpreted_all = true;
        for alternative in &alternatives {
            let hit = match self {
                Self::Ctors { .. } => self.credit_ctors(alternative),
                Self::Product { .. } => self.credit_product(alternative),
                Self::Literals(_) => self.credit_literal(alternative),
                Self::Opaque | Self::TooComplex | Self::Closed => return None,
            };
            match hit {
                Some(hit) => {
                    credit
                        .get_or_insert(Credit {
                            hit: false,
                            interpreted_all: true,
                        })
                        .hit |= hit;
                }
                None => interpreted_all = false,
            }
        }
        credit.map(|credit| Credit {
            interpreted_all,
            ..credit
        })
    }

    /// Credit one alternative against an `EnumLike` root, upstream's
    /// `coverFiniteCtorsCommands`: whether it matched an open constructor,
    /// `None` when it tests no one root constructor.
    fn credit_ctors(&mut self, alternative: &[&CoverCommand]) -> Option<bool> {
        let Self::Ctors {
            names,
            remaining,
            exhaustive,
            universe_covered,
        } = self
        else {
            return None;
        };
        if alternative.iter().all(|command| command.is_bind()) {
            let mut hit = !*exhaustive && !*universe_covered;
            *universe_covered = true;
            for open in remaining.iter_mut() {
                hit |= std::mem::replace(open, false);
            }
            return Some(hit);
        }
        let ctor = root_ctor_match(alternative, names)?;
        if !remaining.get(ctor).copied().unwrap_or(false) {
            return Some(false);
        }
        if simple_root_ctor(alternative, names).is_some() {
            remaining[ctor] = false;
        }
        Some(true)
    }

    /// Credit one alternative against a product, upstream's
    /// `coverProductCommands`: whether it matched an open cell, `None` when
    /// a command constrains no leaf. A binding at or above a leaf leaves it
    /// a wildcard, and a test below one refines the cells it matches
    /// without covering them.
    fn credit_product(&mut self, alternative: &[&CoverCommand]) -> Option<bool> {
        let Self::Product { dims, cells } = self else {
            return None;
        };
        let mut constraints = vec![None; dims.len()];
        let mut refinement = false;
        for command in alternative {
            let path = command.path();
            if let Some(dim) = dims.iter().position(|dim| dim.path == path) {
                if command.is_bind() {
                    continue;
                }
                let ctor = match command {
                    CoverCommand::EnumTag { case, .. } => Some(*case),
                    CoverCommand::Equal { spelling, .. } => {
                        case_index(&dims[dim].names, spelling.as_deref())
                    }
                    _ => None,
                }?;
                if constraints[dim].replace(ctor).is_some() {
                    return None;
                }
            } else if dims.iter().any(|dim| strictly_under(path, &dim.path)) {
                refinement |= !command.is_bind();
            } else if !(command.is_bind() && dims.iter().any(|dim| dim.path.starts_with(path))) {
                return None;
            }
        }
        let mut hit = false;
        for (cell, open) in cells.iter_mut().enumerate() {
            let matched = decode_cell(dims, cell)
                .iter()
                .zip(&constraints)
                .all(|(index, constraint)| constraint.is_none_or(|ctor| ctor == *index));
            if matched && *open {
                hit = true;
                *open = refinement;
            }
        }
        Some(hit)
    }

    /// Credit one alternative against the open literal set, upstream's
    /// `coverLiteralCommands`: a catch-all closes the space, and a root
    /// literal is new or a repeat. `None` for anything else.
    fn credit_literal(&mut self, alternative: &[&CoverCommand]) -> Option<bool> {
        if alternative.iter().all(|command| command.is_bind()) {
            *self = Self::Closed;
            return Some(true);
        }
        let Self::Literals(spellings) = self else {
            return None;
        };
        let literal = simple_root_literal(alternative)?;
        if spellings.iter().any(|spelling| spelling == literal) {
            return Some(false);
        }
        spellings.push(literal.to_string());
        Some(true)
    }

    /// The value an unreachable case without `|` repeats, as its warning
    /// names it: a root constructor or a root literal.
    fn covered_value(&self, commands: &[CoverCommand]) -> Option<String> {
        if commands
            .iter()
            .any(|command| matches!(command, CoverCommand::Or(_)))
        {
            return None;
        }
        let commands: Vec<&CoverCommand> = commands.iter().collect();
        match self {
            Self::Ctors { names, .. } => root_ctor_match(&commands, names)
                .map(|ctor| names.get(ctor).map_or("?", String::as_str).to_string()),
            Self::Literals(_) => simple_root_literal(&commands).map(str::to_string),
            _ => None,
        }
    }

    /// The match-level warning once every case is credited: a product too
    /// large to track, or the values of a closed space no case covers.
    fn match_warning(&self) -> Option<String> {
        let missing: Vec<String> = match self {
            Self::TooComplex => {
                return Some(
                    "'match' is too complex to check for exhaustivity; add a '_' case".to_string(),
                );
            }
            Self::Ctors {
                names,
                remaining,
                exhaustive: true,
                ..
            } => names
                .iter()
                .zip(remaining)
                .filter(|(_, open)| **open)
                .map(|(name, _)| name.clone())
                .collect(),
            Self::Product { dims, cells } => cells
                .iter()
                .enumerate()
                .filter(|(_, open)| **open)
                .map(|(cell, _)| witness(dims, cell))
                .collect(),
            _ => return None,
        };
        let shown = &missing[..missing.len().min(MAX_SUGGESTED)];
        (!missing.is_empty()).then(|| missing_message(shown, missing.len()))
    }
}

/// One leaf of a product: its path from the subject and its case names.
struct Dim {
    path: Vec<Step>,
    names: Vec<String>,
}

/// An `EnumLike`'s constructors as its members fold: unknown names or an
/// unknown exhaustiveness flag are `None`.
struct Constructors {
    names: Option<Vec<String>>,
    exhaustive: Option<bool>,
}

/// How a case's interpreted alternatives fared: whether any matched a
/// value still open, and whether every alternative could be interpreted.
#[derive(Clone, Copy)]
struct Credit {
    hit: bool,
    interpreted_all: bool,
}

/// The warning `message` at `span`.
fn warning(message: String, span: &SourceSpan) -> Warning {
    Warning {
        message,
        span: span.clone(),
    }
}

/// The rejection of an `EnumLike` whose `_enum_is_exhaustive` does not
/// fold.
fn exhaustiveness_not_static() -> TypeError {
    TypeError::MatchPattern(
        "'_enum_is_exhaustive' must be statically known when matching an EnumLike type".into(),
    )
}

/// The cell count of a product of `ctors` constructors per leaf, `None`
/// above `MAX_CELLS`; each step is checked before it multiplies, so no
/// count overflows.
fn product_cells(ctors: &[usize]) -> Option<usize> {
    ctors.iter().try_fold(1usize, |cells, &ctors| {
        (ctors != 0 && cells <= MAX_CELLS / ctors).then(|| cells * ctors)
    })
}

/// The one root constructor an alternative matches within, upstream's
/// `getRootCtorMatch`: a root tag test or a root literal naming a case,
/// tests below the root (a payload's) narrowing it without changing it.
fn root_ctor_match(alternative: &[&CoverCommand], names: &[String]) -> Option<usize> {
    let mut ctor = None;
    for command in alternative {
        if command.is_bind() || !command.path().is_empty() {
            continue;
        }
        let found = match command {
            CoverCommand::EnumTag { case, .. } => *case,
            CoverCommand::Equal { spelling, .. } => case_index(names, spelling.as_deref())?,
            _ => return None,
        };
        if ctor.replace(found).is_some() {
            return None;
        }
    }
    ctor
}

/// The root constructor an alternative covers whole, upstream's
/// `getSimpleRootCtorCoverage`: its only test is the root's, so `Some(_)`
/// covers `Some` and `Some(0)` does not.
fn simple_root_ctor(alternative: &[&CoverCommand], names: &[String]) -> Option<usize> {
    root_ctor_match(alternative, names).filter(|_| {
        alternative
            .iter()
            .all(|command| command.is_bind() || command.path().is_empty())
    })
}

/// The spelling of an alternative's one root literal test, upstream's
/// `getSimpleRootLiteralCoverage`, bindings aside.
fn simple_root_literal<'c>(alternative: &[&'c CoverCommand]) -> Option<&'c str> {
    let mut literal = None;
    for command in alternative {
        match command {
            CoverCommand::Bind { .. } => {}
            CoverCommand::Equal {
                path,
                spelling: Some(spelling),
            } if path.is_empty() && literal.is_none() => literal = Some(spelling.as_str()),
            _ => return None,
        }
    }
    literal
}

/// The index of the case a literal's `spelling` names (`True`).
fn case_index(names: &[String], spelling: Option<&str>) -> Option<usize> {
    let spelling = spelling.filter(|spelling| !spelling.is_empty())?;
    names.iter().position(|name| name == spelling)
}

/// Whether `path` lies strictly below `ancestor`.
fn strictly_under(path: &[Step], ancestor: &[Step]) -> bool {
    path.len() > ancestor.len() && path.starts_with(ancestor)
}

/// The `|`-free alternatives `commands` stand for, upstream's
/// `expandOrCommands`: the first `|` substituted by each of its
/// alternatives in turn, expanded again.
fn expand_or<'c>(commands: Vec<&'c CoverCommand>, out: &mut Vec<Vec<&'c CoverCommand>>) {
    let first_or = commands
        .iter()
        .enumerate()
        .find_map(|(at, command)| match command {
            CoverCommand::Or(alternatives) => Some((at, alternatives)),
            _ => None,
        });
    let Some((at, alternatives)) = first_or else {
        out.push(commands);
        return;
    };
    for alternative in alternatives {
        let combined = commands[..at]
            .iter()
            .copied()
            .chain(alternative)
            .chain(commands[at + 1..].iter().copied())
            .collect();
        expand_or(combined, out);
    }
}

/// A product cell as its constructor index per leaf, the last leaf
/// varying fastest.
fn decode_cell(dims: &[Dim], mut cell: usize) -> Vec<usize> {
    let mut indices = vec![0; dims.len()];
    for (index, dim) in dims.iter().enumerate().rev() {
        indices[index] = cell % dim.names.len();
        cell /= dim.names.len();
    }
    indices
}

/// A product cell as the missing-case warning spells it: its leaves' case
/// names, joined (`False, True`).
fn witness(dims: &[Dim], cell: usize) -> String {
    decode_cell(dims, cell)
        .iter()
        .zip(dims)
        .map(|(index, dim)| dim.names[*index].as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The missing-case warning naming the `shown` values of `count` missing:
/// one alone, two joined by `and`, more with an Oxford comma, and the
/// values past the shown ones counted (`, and 7 more`).
fn missing_message(shown: &[String], count: usize) -> String {
    let mut message = format!(
        "'match' is not exhaustive; missing case{}",
        if shown.len() == 1 { " for " } else { "s for " }
    );
    for (index, name) in shown.iter().enumerate() {
        if index != 0 {
            message.push_str(if index + 1 != shown.len() || count > shown.len() {
                ", "
            } else if shown.len() == 2 {
                " and "
            } else {
                ", and "
            });
        }
        message.push('\'');
        message.push_str(name);
        message.push('\'');
    }
    if count > shown.len() {
        message = format!("{message}, and {} more", count - shown.len());
    }
    message
}

#[cfg(test)]
mod tests {
    use super::{MAX_CELLS, missing_message, product_cells};

    #[test]
    fn product_cells_caps_at_1024_cells() {
        assert_eq!(product_cells(&[2; 10]), Some(MAX_CELLS));
        assert_eq!(product_cells(&[2; 11]), None);
        assert_eq!(product_cells(&[3, 2]), Some(6));
    }

    #[test]
    fn product_cells_rejects_overflow_before_allocating() {
        assert_eq!(product_cells(&[usize::MAX, 2]), None);
        assert_eq!(product_cells(&[1 << 40, 1 << 40]), None);
        assert_eq!(product_cells(&[2, 0]), None);
    }

    #[test]
    fn missing_message_lists_eight_then_counts_the_rest() {
        let names: Vec<String> = (0..15).map(|index| format!("c{index}")).collect();
        assert_eq!(
            missing_message(&names[..8], 15),
            "'match' is not exhaustive; missing cases for 'c0', 'c1', 'c2', 'c3', 'c4', \
             'c5', 'c6', 'c7', and 7 more"
        );
        assert_eq!(
            missing_message(&names[..1], 1),
            "'match' is not exhaustive; missing case for 'c0'"
        );
        assert_eq!(
            missing_message(&names[..2], 2),
            "'match' is not exhaustive; missing cases for 'c0' and 'c1'"
        );
        assert_eq!(
            missing_message(&names[..3], 3),
            "'match' is not exhaustive; missing cases for 'c0', 'c1', and 'c2'"
        );
    }
}
