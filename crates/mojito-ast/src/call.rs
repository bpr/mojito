//! Phase-neutral function-call argument binding.
//!
//! Checking, MIR declaration lowering, and runtime frame construction share
//! this structural contract. It deliberately knows nothing about types or
//! values; each phase interprets the matched slots. The error adaptations
//! kept here are `MatchError::into_type_error` and
//! `ParamMatchError::into_type_error`, the checker-facing renderings that
//! must live beside the types they convert.

use crate::ast::{DeclaredParam, ParamArg, ParamKind};
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
/// `1` to `a`.
pub fn match_parameter_slots<P: CompileTimeParam>(
    params: &[P],
    keywords: &[Option<&str>],
) -> Result<Vec<usize>, ParamMatchError> {
    let mut slots = vec![None; keywords.len()];
    let mut filled = vec![false; params.len()];
    let mut next = 0;
    let mut skipped_infer_only = false;
    for (slot, _) in slots
        .iter_mut()
        .zip(keywords)
        .filter(|(_, keyword)| keyword.is_none())
    {
        while params.get(next).is_some_and(CompileTimeParam::infer_only) {
            skipped_infer_only = true;
            next += 1;
        }
        let Some(param) = params.get(next) else {
            return Err(ParamMatchError::UnexpectedParameter {
                skipped_infer_only,
                expected: params.iter().filter(|param| !param.infer_only()).count(),
                got: keywords.len(),
            });
        };
        *slot = Some(next);
        filled[next] = true;
        if !param.variadic() {
            next += 1;
        }
    }
    for (slot, keyword) in slots.iter_mut().zip(keywords) {
        let Some(keyword) = keyword else {
            continue;
        };
        let index = params
            .iter()
            .position(|param| param.keyword() == *keyword)
            .ok_or_else(|| ParamMatchError::UnknownKeyword((*keyword).to_string()))?;
        if filled[index] {
            return Err(ParamMatchError::UnexpectedKeyword((*keyword).to_string()));
        }
        filled[index] = true;
        *slot = Some(index);
    }
    Ok(slots.into_iter().flatten().collect())
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
