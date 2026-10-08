//! The per-instance report of a method clone whose instantiation failed.
//!
//! The elaborator above MIR mints a method's clone for every instance,
//! called or not. A clone whose body fails to elaborate there keeps the
//! instance's signature and a body that is the one call
//! `_mojito_instantiation_failed("…")`. Upstream instantiates a function
//! only where the program reaches it, so the failure is reported here, per
//! reached instance once its compile-time branches are decided: an untaken
//! arm's marker is never reported.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Const;

impl Specializer<'_> {
    /// Fail the instance `function` of `template` if a block its entry
    /// reaches calls `_mojito_instantiation_failed`, with that call's
    /// message.
    pub(super) fn discharge_instantiation_failures(
        &self,
        template: &str,
        function: &MirFunction,
    ) -> Result<(), MonoError> {
        let reached = rebind::reachable(&function.blocks);
        let instructions = || {
            function
                .blocks
                .iter()
                .zip(&reached)
                .filter(|(_, reached)| **reached)
                .flat_map(|(block, _)| &block.instrs)
        };
        let Some(message) = instructions().find_map(|instruction| match instruction {
            MirInstr::Call { func, args, .. } if func.0 == INSTANTIATION_FAILED => args.first(),
            _ => None,
        }) else {
            return Ok(());
        };
        let construct = instructions()
            .find_map(|instruction| match instruction {
                MirInstr::Const {
                    dest,
                    k: Const::Str(text),
                } if dest == message => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Err(MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(Some(template), construct)
        })
    }
}

/// The failure of a parameter constant an unrolled iteration could not
/// compute ([`Specializer::answer_param_constants`]) whose read the
/// instance still holds once its branches are decided and its unreached
/// blocks pruned.
pub(super) fn reached_failure(
    blocks: &[MirBlock],
    mut failures: HashMap<u32, MonoError>,
) -> Result<(), MonoError> {
    if failures.is_empty() {
        return Ok(());
    }
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .try_for_each(|instruction| match instruction {
            MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } => std::iter::once(body)
                .chain(handler.iter().map(|(_, blocks)| blocks))
                .chain(orelse)
                .chain(finalbody)
                .try_for_each(|region| reached_failure(region, failures.clone())),
            MirInstr::Const {
                dest,
                k: Const::Param(_),
            } => failures.remove(&dest.0).map_or(Ok(()), Err),
            _ => Ok(()),
        })
}

/// The compiler-private call a failed method clone's body makes.
const INSTANTIATION_FAILED: &str = "_mojito_instantiation_failed";
