//! Phase-neutral function-call argument binding.
//!
//! Checking, MIR declaration lowering, and runtime frame construction share
//! this structural contract. It deliberately knows nothing about types or
//! values; each phase interprets the matched slots. The error adaptations
//! kept here are `MatchError::into_type_error` and
//! `ParamMatchError::into_type_error`, the checker-facing renderings that
//! must live beside the types they convert.

use crate::ast::{DeclaredParam, ExprKind, ParamArg, ParamKind, Type};
use mojito_common::error::TypeError;

#[derive(Clone, Copy)]
/// Source argument selected for one regular parameter slot.
pub enum ArgSlot {
    Positional(usize),
    Keyword(usize),
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structural reason an argument list cannot bind to a declaration.
pub enum MatchError {
    TooManyPositional {
        expected: usize,
        got: usize,
    },
    UnknownKeyword(String),
    PositionalOnly(String),
    Duplicate(String),
    Missing(String),
    /// More than one `*pack` among the positional arguments.
    RepeatedSpread,
    /// A positional argument after the `*pack`.
    PositionalAfterSpread,
    /// The `*pack` did not land alone in the callee's positional collector:
    /// it filled a regular slot, or shares the collector with another
    /// argument.
    SpreadOutsideVariadic,
    /// A forwarded `**kwargs^` shares the callee's keyword collector with
    /// a keyword argument no named parameter claims.
    KeywordsBesideForward,
}

#[derive(Clone, Copy)]
/// Whether positional and keyword overflow collectors are present.
pub struct CallVariadics {
    pub positional: bool,
    pub keyword: bool,
}

/// Checker-facing adaptation: the structural mismatch as a `TypeError`.
impl MatchError {
    pub fn into_type_error(self, func: &str) -> TypeError {
        match self {
            Self::TooManyPositional { expected, got } => TypeError::ArityMismatch {
                name: func.to_string(),
                expected,
                got,
            },
            Self::UnknownKeyword(k) => TypeError::BadCall {
                func: func.to_string(),
                reason: format!("unexpected keyword argument '{k}'"),
            },
            Self::PositionalOnly(k) => TypeError::BadCall {
                func: func.to_string(),
                reason: format!("argument '{k}' is positional-only"),
            },
            Self::Duplicate(k) => TypeError::BadCall {
                func: func.to_string(),
                reason: format!("argument '{k}' supplied more than once"),
            },
            Self::Missing(m) => TypeError::BadCall {
                func: func.to_string(),
                reason: format!("missing required argument '{m}'"),
            },
            Self::RepeatedSpread => TypeError::BadCall {
                func: func.to_string(),
                reason: "unpack markers must not appear more than once in a call".to_string(),
            },
            Self::PositionalAfterSpread => TypeError::BadCall {
                func: func.to_string(),
                reason: "positional argument must not follow an unpack".to_string(),
            },
            Self::SpreadOutsideVariadic => TypeError::BadCall {
                func: func.to_string(),
                reason: "unpack is only supported when the callee accepts a variadic pack"
                    .to_string(),
            },
            Self::KeywordsBesideForward => TypeError::BadCall {
                func: func.to_string(),
                reason: "combining a '**' unpack with other keyword arguments for '**kwargs' \
                         is not supported"
                    .to_string(),
            },
        }
    }
}
/// Fully matched regular slots plus variadic overflow source indexes.
pub struct CallSlots {
    pub slots: Vec<ArgSlot>,
    pub positional_overflow: Vec<usize>,
    pub keyword_overflow: Vec<usize>,
}

/// One declared compile-time parameter, as bracket binding sees it.
pub trait CompileTimeParam {
    /// The name a keyword argument spells, without a pack's `*`.
    fn keyword(&self) -> &str;
    /// Whether the parameter is declared before `//`.
    fn infer_only(&self) -> bool;
    /// Whether the parameter is a pack (`*Ts`, `*values: Int`).
    fn variadic(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structural reason a compile-time bracket list cannot bind to a
/// declaration's parameters.
pub enum ParamMatchError {
    /// A positional argument left over once every slot that is not
    /// infer-only is full.
    UnexpectedParameter {
        skipped_infer_only: bool,
        expected: usize,
        got: usize,
    },
    /// A keyword naming a slot another argument already filled.
    UnexpectedKeyword(String),
    UnknownKeyword(String),
    /// A `...` that is not the list's last, positional argument.
    MisplacedRest,
}

/// Checker-facing adaptation: the structural mismatch as a `TypeError`.
impl ParamMatchError {
    pub fn into_type_error(self, name: &str) -> TypeError {
        match self {
            Self::UnexpectedParameter {
                skipped_infer_only: true,
                ..
            } => TypeError::BadCall {
                func: name.to_string(),
                reason: "unexpected parameter".to_string(),
            },
            Self::UnexpectedParameter { expected, got, .. } => TypeError::WrongTypeArgCount {
                name: name.to_string(),
                expected,
                got,
            },
            Self::UnexpectedKeyword(keyword) => TypeError::BadCall {
                func: name.to_string(),
                reason: format!("unexpected keyword parameter '{keyword}'"),
            },
            Self::UnknownKeyword(keyword) => TypeError::Unsupported(format!(
                "generic '{name}' has no parameter named '{keyword}'"
            )),
            Self::MisplacedRest => TypeError::BadCall {
                func: name.to_string(),
                reason: "'...' must be the last parameter".to_string(),
            },
        }
    }
}

/// The one `*pack` among `npos` positional arguments, given the positions
/// of every spread: it must be the last positional argument and the only
/// spread. `None` when the call spreads nothing.
pub fn spread_position(spreads: &[usize], npos: usize) -> Result<Option<usize>, MatchError> {
    match spreads {
        [] => Ok(None),
        [spread] if *spread + 1 == npos => Ok(Some(*spread)),
        [_] => Err(MatchError::PositionalAfterSpread),
        _ => Err(MatchError::RepeatedSpread),
    }
}

/// Whether the `*pack` at `spread` bound to the callee's positional
/// collector by itself: the whole forwarded pack is that collector's one
/// argument, never a regular slot's.
pub fn bind_spread(matched: &CallSlots, spread: usize) -> Result<(), MatchError> {
    if matched.positional_overflow == [spread] {
        Ok(())
    } else {
        Err(MatchError::SpreadOutsideVariadic)
    }
}

/// Whether a forwarded `**kwargs^` bound the callee's keyword collector alone.
///
/// `forwarded` says whether the call forwards one: as upstream's call passes
/// the forwarded dictionary whole, no keyword argument may share the
/// collector with it.
pub const fn bind_keyword_forward(matched: &CallSlots, forwarded: bool) -> Result<(), MatchError> {
    if forwarded && !matched.keyword_overflow.is_empty() {
        Err(MatchError::KeywordsBesideForward)
    } else {
        Ok(())
    }
}

/// Match positional and keyword arguments to regular parameter slots.
pub fn match_call_slots(
    param_names: &[String],
    required: &[bool],
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    npos: usize,
    kw_names: &[&str],
    variadics: CallVariadics,
) -> Result<CallSlots, MatchError> {
    let nparams = param_names.len();
    debug_assert_eq!(required.len(), nparams);
    let positional_limit = keyword_only.unwrap_or(nparams).min(nparams);
    if npos > positional_limit && !variadics.positional {
        return Err(MatchError::TooManyPositional {
            expected: positional_limit,
            got: npos + kw_names.len(),
        });
    }
    let n_regular_pos = npos.min(positional_limit);
    let positional_overflow = if variadics.positional {
        (positional_limit..npos).collect()
    } else {
        Vec::new()
    };
    let mut slots = vec![None; nparams];
    let mut keyword_overflow = Vec::new();
    for (index, slot) in slots.iter_mut().enumerate().take(n_regular_pos) {
        *slot = Some(ArgSlot::Positional(index));
    }
    for (keyword_index, keyword) in kw_names.iter().enumerate() {
        if kw_names[..keyword_index].contains(keyword) {
            return Err(MatchError::Duplicate((*keyword).to_string()));
        }
        let Some(parameter_index) = param_names.iter().position(|name| name == keyword) else {
            if variadics.keyword {
                keyword_overflow.push(keyword_index);
                continue;
            }
            return Err(MatchError::UnknownKeyword((*keyword).to_string()));
        };
        if positional_only.is_some_and(|limit| parameter_index < limit) {
            return Err(MatchError::PositionalOnly((*keyword).to_string()));
        }
        if slots[parameter_index].is_some() {
            return Err(MatchError::Duplicate((*keyword).to_string()));
        }
        slots[parameter_index] = Some(ArgSlot::Keyword(keyword_index));
    }
    let mut matched = Vec::with_capacity(nparams);
    for (index, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(slot) => matched.push(slot),
            None if required[index] => {
                return Err(MatchError::Missing(param_names[index].clone()));
            }
            None => matched.push(ArgSlot::Default),
        }
    }
    Ok(CallSlots {
        slots: matched,
        positional_overflow,
        keyword_overflow,
    })
}

/// Match a `@fieldwise_init` construction's arguments to the struct's fields.
///
/// The synthesized constructor takes one required parameter per field, named
/// after it, in declaration order, so every returned slot is positional or
/// keyword, never a default.
pub fn match_fieldwise_slots(
    field_names: &[String],
    npos: usize,
    kw_names: &[&str],
) -> Result<Vec<ArgSlot>, MatchError> {
    match_call_slots(
        field_names,
        &vec![true; field_names.len()],
        None,
        None,
        npos,
        kw_names,
        CallVariadics {
            positional: false,
            keyword: false,
        },
    )
    .map(|matched| matched.slots)
}

/// Convert a parser marker to the regular-parameter index space used by
/// calls. A named `out` result is not part of that space: the caller never
/// supplies it.
#[allow(
    clippy::single_option_map,
    reason = "all 14 call sites thread Option<usize> through unchanged"
)]
pub fn regular_marker_index<P: DeclaredParam>(
    params: &[P],
    marker: Option<usize>,
) -> Option<usize> {
    marker.map(|index| {
        params[..index]
            .iter()
            .filter(|parameter| {
                parameter.kind() == ParamKind::Regular && !parameter.is_named_result()
            })
            .count()
    })
}

/// Effective keyword-only boundary after combining `*` and `*args` markers.
pub fn effective_keyword_only_index<P: DeclaredParam>(
    params: &[P],
    keyword_only: Option<usize>,
    variadic_index: Option<usize>,
) -> Option<usize> {
    [
        regular_marker_index(params, keyword_only),
        regular_marker_index(params, variadic_index),
    ]
    .into_iter()
    .flatten()
    .min()
}

/// The parameter each compile-time bracket argument binds, in argument
/// order, given each argument's keyword (`None` for a positional one).
///
/// Positional arguments bind first, in order, each to the next parameter
/// that is not infer-only; a pack takes every positional argument from its
/// slot on. Each keyword then binds its parameter by name, an infer-only one
/// included. Where a keyword is written plays no part, so `C[b=2, 1]` binds
/// `1` to `a`. The first positional mismatch is reported before any keyword
/// one.
pub fn match_parameter_slots<P: CompileTimeParam>(
    params: &[P],
    keywords: &[Option<&str>],
) -> Result<Vec<usize>, ParamMatchError> {
    let bound = bind_parameter_slots(params, keywords);
    let mut positional_first = bound
        .iter()
        .zip(keywords)
        .filter(|(_, keyword)| keyword.is_none())
        .chain(
            bound
                .iter()
                .zip(keywords)
                .filter(|(_, keyword)| keyword.is_some()),
        );
    if let Some(error) = positional_first.find_map(|(slot, _)| slot.as_ref().err()) {
        return Err(error.clone());
    }
    Ok(bound.into_iter().flatten().collect())
}

/// Per bracket argument, the parameter it binds by
/// [`match_parameter_slots`]' rule, or why it binds none.
///
/// A consumer that tolerates an argument binding nothing (a lowered list's
/// erased origin argument) reads each result on its own; one argument's
/// mismatch leaves every other binding as the rule makes it.
pub fn bind_parameter_slots<P: CompileTimeParam>(
    params: &[P],
    keywords: &[Option<&str>],
) -> Vec<Result<usize, ParamMatchError>> {
    let expected = params.iter().filter(|param| !param.infer_only()).count();
    let mut filled = vec![false; params.len()];
    let mut next = 0;
    let mut skipped_infer_only = false;
    let positional: Vec<Option<Result<usize, ParamMatchError>>> = keywords
        .iter()
        .map(|keyword| {
            if keyword.is_some() {
                return None;
            }
            while params.get(next).is_some_and(CompileTimeParam::infer_only) {
                skipped_infer_only = true;
                next += 1;
            }
            let Some(param) = params.get(next) else {
                return Some(Err(ParamMatchError::UnexpectedParameter {
                    skipped_infer_only,
                    expected,
                    got: keywords.len(),
                }));
            };
            let index = next;
            filled[index] = true;
            next += usize::from(!param.variadic());
            Some(Ok(index))
        })
        .collect();
    positional
        .into_iter()
        .zip(keywords)
        .map(|(slot, keyword)| {
            slot.unwrap_or_else(|| {
                let keyword = keyword.unwrap_or_default();
                match params.iter().position(|param| param.keyword() == keyword) {
                    None => Err(ParamMatchError::UnknownKeyword(keyword.to_string())),
                    Some(index) if filled[index] => {
                        Err(ParamMatchError::UnexpectedKeyword(keyword.to_string()))
                    }
                    Some(index) => {
                        filled[index] = true;
                        Ok(index)
                    }
                }
            })
        })
        .collect()
}

/// How a bracket argument leaves a parameter unbound: a `_` its own slot,
/// a trailing `...` every slot the arguments before it leave, defaulted
/// ones included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unbound {
    Hole,
    Rest,
}

/// The unbound marker `argument` spells itself (`_` or `...`, in either
/// spelling the parser gives it), a keyword argument's value not looked
/// through.
pub fn unbound_marker(argument: &ParamArg) -> Option<Unbound> {
    let name = match argument {
        ParamArg::Value(value) => match &value.kind {
            ExprKind::Identifier(name) => name,
            _ => return None,
        },
        ParamArg::Type(Type::Named(name, args)) if args.is_empty() => name,
        ParamArg::Type(_) | ParamArg::Named { .. } => return None,
    };
    match name.as_str() {
        "_" => Some(Unbound::Hole),
        "..." => Some(Unbound::Rest),
        _ => None,
    }
}

/// A bracket list's binding with its unbound markers taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnboundSlots {
    /// How many leading arguments bind a slot, `_` included: the list
    /// without its trailing `...`.
    pub listed: usize,
    /// The positions of the arguments that bind a value.
    pub kept: Vec<usize>,
    /// Per parameter, the marker that leaves it unbound, if any.
    pub open: Vec<Option<Unbound>>,
}

impl UnboundSlots {
    /// Whether the list leaves any parameter unbound.
    pub fn any_open(&self) -> bool {
        self.open.iter().any(Option::is_some)
    }
}

/// The parameters a bracket list leaves unbound.
///
/// As upstream's parameter inference marks them, a `_` (positional or
/// keyword) leaves its own slot unbound, installing no default there, and
/// a trailing `...` every slot that is not infer-only and no other argument
/// fills.
pub fn bracket_unbound_slots<P: CompileTimeParam>(
    params: &[P],
    arguments: &[ParamArg],
) -> Result<UnboundSlots, ParamMatchError> {
    let marker = |argument: &ParamArg| match argument {
        ParamArg::Named { value, .. } => unbound_marker(value),
        positional => unbound_marker(positional),
    };
    let rest = matches!(arguments.last(), Some(last @ (ParamArg::Value(_) | ParamArg::Type(_)))
        if unbound_marker(last) == Some(Unbound::Rest));
    let bound = &arguments[..arguments.len() - usize::from(rest)];
    if bound
        .iter()
        .any(|argument| marker(argument) == Some(Unbound::Rest))
    {
        return Err(ParamMatchError::MisplacedRest);
    }
    let slots = match_parameter_slots(params, &bracket_keywords(bound))?;
    let mut open = vec![None; params.len()];
    let mut filled = vec![false; params.len()];
    let mut kept = Vec::with_capacity(bound.len());
    for (position, (argument, slot)) in bound.iter().zip(slots).enumerate() {
        filled[slot] = true;
        if marker(argument) == Some(Unbound::Hole) {
            open[slot] = Some(Unbound::Hole);
        } else {
            kept.push(position);
        }
    }
    if rest {
        for (index, param) in params.iter().enumerate() {
            if !filled[index] && !param.infer_only() {
                open[index] = Some(Unbound::Rest);
            }
        }
    }
    Ok(UnboundSlots {
        listed: bound.len(),
        kept,
        open,
    })
}

/// Each source bracket argument's keyword, as [`match_parameter_slots`]
/// reads it.
pub fn bracket_keywords(arguments: &[ParamArg]) -> Vec<Option<&str>> {
    arguments
        .iter()
        .map(|argument| match argument {
            ParamArg::Named { name, .. } => Some(name.as_str()),
            ParamArg::Type(_) | ParamArg::Value(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Param(&'static str, bool, bool);

    impl CompileTimeParam for Param {
        fn keyword(&self) -> &str {
            self.0
        }

        fn infer_only(&self) -> bool {
            self.1
        }

        fn variadic(&self) -> bool {
            self.2
        }
    }

    #[test]
    fn match_parameter_slots_binds_positionals_before_keywords() {
        let params = [Param("a", false, false), Param("b", false, false)];
        assert_eq!(
            match_parameter_slots(&params, &[Some("b"), None]),
            Ok(vec![1, 0])
        );
        assert_eq!(
            match_parameter_slots(&params, &[Some("a"), None]),
            Err(ParamMatchError::UnexpectedKeyword("a".to_string()))
        );
    }

    #[test]
    fn match_parameter_slots_binds_an_infer_only_parameter_by_keyword() {
        let params = [Param("T", true, false), Param("n", false, false)];
        assert_eq!(
            match_parameter_slots(&params, &[Some("T"), Some("n")]),
            Ok(vec![0, 1])
        );
        assert_eq!(
            match_parameter_slots(&params, &[Some("T"), None]),
            Ok(vec![0, 1])
        );
        assert_eq!(
            match_parameter_slots(&params, &[None, None]),
            Err(ParamMatchError::UnexpectedParameter {
                skipped_infer_only: true,
                expected: 1,
                got: 2,
            })
        );
    }

    #[test]
    fn match_parameter_slots_collects_positionals_after_a_keyword_into_a_pack() {
        let params = [Param("T", true, false), Param("vs", false, true)];
        assert_eq!(
            match_parameter_slots(&params, &[Some("T"), None, None]),
            Ok(vec![0, 1, 1])
        );
    }

    #[test]
    fn match_parameter_slots_rejects_a_surplus_or_unknown_argument() {
        let params = [Param("a", false, false)];
        assert_eq!(
            match_parameter_slots(&params, &[None, None]),
            Err(ParamMatchError::UnexpectedParameter {
                skipped_infer_only: false,
                expected: 1,
                got: 2,
            })
        );
        assert_eq!(
            match_parameter_slots(&params, &[Some("z")]),
            Err(ParamMatchError::UnknownKeyword("z".to_string()))
        );
    }

    #[test]
    fn match_parameter_slots_rejects_a_keyword_for_a_positionally_filled_slot() {
        let params = [Param("a", false, false), Param("b", false, false)];
        assert_eq!(
            match_parameter_slots(&params, &[Some("a"), None]),
            Err(ParamMatchError::UnexpectedKeyword("a".to_string()))
        );
    }

    #[test]
    fn bind_parameter_slots_keeps_each_binding_beside_an_unmatched_argument() {
        let params = [Param("T", true, false), Param("a", false, false)];
        assert_eq!(
            bind_parameter_slots(&params, &[None, Some("o"), Some("T"), None]),
            vec![
                Ok(1),
                Err(ParamMatchError::UnknownKeyword("o".to_string())),
                Ok(0),
                Err(ParamMatchError::UnexpectedParameter {
                    skipped_infer_only: true,
                    expected: 1,
                    got: 4,
                }),
            ]
        );
    }

    fn bracket(source: &[&str]) -> Vec<ParamArg> {
        source
            .iter()
            .map(|argument| {
                let (name, value) = match argument.split_once('=') {
                    Some((name, value)) => (Some(name), value),
                    None => (None, *argument),
                };
                let value = ParamArg::Value(crate::ast::Expr::new(
                    ExprKind::Identifier(value.to_string()),
                    mojito_common::token::Span::default(),
                ));
                match name {
                    Some(name) => ParamArg::Named {
                        name: name.to_string(),
                        value: Box::new(value),
                    },
                    None => value,
                }
            })
            .collect()
    }

    fn unbound(listed: usize, kept: &[usize], open: &str) -> UnboundSlots {
        let open = open
            .chars()
            .map(|slot| match slot {
                '_' => Some(Unbound::Hole),
                '.' => Some(Unbound::Rest),
                _ => None,
            })
            .collect();
        UnboundSlots {
            listed,
            kept: kept.to_vec(),
            open,
        }
    }

    #[test]
    fn bracket_unbound_slots_marks_a_hole_and_a_trailing_rest() {
        let params = [
            Param("a", false, false),
            Param("b", false, false),
            Param("c", false, false),
        ];
        let slots = |source: &[&str]| bracket_unbound_slots(&params, &bracket(source));
        assert_eq!(slots(&["x", "_", "y"]), Ok(unbound(3, &[0, 2], "x_x")));
        assert_eq!(slots(&["b=x", "..."]), Ok(unbound(1, &[0], ".x.")));
        assert_eq!(slots(&["..."]), Ok(unbound(0, &[], "...")));
        assert_eq!(slots(&["_", "x", "..."]), Ok(unbound(2, &[1], "_x.")));
        assert_eq!(
            slots(&["a=_", "b=x", "c=y"]),
            Ok(unbound(3, &[1, 2], "_xx"))
        );
        assert_eq!(slots(&["...", "x"]), Err(ParamMatchError::MisplacedRest));
        assert_eq!(slots(&["b=..."]), Err(ParamMatchError::MisplacedRest));
    }

    #[test]
    fn bracket_unbound_slots_leaves_an_infer_only_parameter_to_inference() {
        let params = [Param("T", true, false), Param("a", false, false)];
        assert_eq!(
            bracket_unbound_slots(&params, &bracket(&["..."])),
            Ok(unbound(0, &[], "x."))
        );
    }

    fn names() -> Vec<String> {
        ["a", "b", "c"].map(str::to_string).to_vec()
    }

    #[test]
    fn matches_mixed_arguments_defaults_and_both_overflows() {
        let matched = match_call_slots(
            &names(),
            &[true, false, true],
            None,
            Some(2),
            3,
            &["c", "extra"],
            CallVariadics {
                positional: true,
                keyword: true,
            },
        )
        .unwrap();

        assert!(matches!(matched.slots[0], ArgSlot::Positional(0)));
        assert!(matches!(matched.slots[1], ArgSlot::Positional(1)));
        assert!(matches!(matched.slots[2], ArgSlot::Keyword(0)));
        assert_eq!(matched.positional_overflow, [2]);
        assert_eq!(matched.keyword_overflow, [1]);
    }

    #[test]
    fn rejects_keyword_binding_to_a_positional_only_parameter() {
        let error = match_call_slots(
            &names(),
            &[true, false, false],
            Some(1),
            None,
            0,
            &["a"],
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .err()
        .unwrap();
        assert_eq!(error, MatchError::PositionalOnly("a".to_string()));
    }

    #[test]
    fn rejects_duplicate_keyword_collector_entries() {
        let error = match_call_slots(
            &[],
            &[],
            None,
            None,
            0,
            &["extra", "extra"],
            CallVariadics {
                positional: false,
                keyword: true,
            },
        )
        .err()
        .expect("collector keys must remain unique");
        assert_eq!(error, MatchError::Duplicate("extra".to_string()));
    }

    #[test]
    fn a_spread_is_the_last_and_only_positional_unpack() {
        assert_eq!(spread_position(&[], 2), Ok(None));
        assert_eq!(spread_position(&[2], 3), Ok(Some(2)));
        assert_eq!(
            spread_position(&[1], 3),
            Err(MatchError::PositionalAfterSpread)
        );
        assert_eq!(spread_position(&[0, 1], 2), Err(MatchError::RepeatedSpread));
    }

    #[test]
    fn a_spread_binds_alone_to_the_positional_collector() {
        let variadics = CallVariadics {
            positional: true,
            keyword: false,
        };
        let matched = match_call_slots(&names()[..1], &[true], None, None, 2, &[], variadics)
            .expect("one regular slot plus the spread");
        assert_eq!(bind_spread(&matched, 1), Ok(()));
        let filled = match_call_slots(&names()[..2], &[true, true], None, None, 2, &[], variadics)
            .expect("the spread fills the second regular slot");
        assert_eq!(
            bind_spread(&filled, 1),
            Err(MatchError::SpreadOutsideVariadic)
        );
    }
}
