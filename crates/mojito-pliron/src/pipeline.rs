//! The profile-to-pipeline table: the single owner of native optimization
//! policy. Both profiles share the pliron cleanup stage (per-function
//! mem2reg then DCE, run between module verifications by `compile`);
//! profiles diverge only at the LLVM stage, where `release` runs the pinned
//! `default<O1>` pipeline through an external `opt` over emitted bitcode
//! (pliron-llvm 0.17 keeps its raw `LLVMModuleRef` private, so the
//! new-pass-manager is unreachable in-process). Changing a pass list, the
//! LLVM pipeline string, or the `opt` invocation shape is a reviewed policy
//! change and must update the snapshot tests below.

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::op::Op;
use pliron::pass::{AnalysisManager, NestedOpsPass, OpPass, Pass, Passes};
use pliron::printable::Printable;

use super::{OptLevel, PlironError, PlironErrorKind};

/// Run `work` as one phase of the `--timings` report (the bench driver's
/// per-phase compile-timing channel, keyed by this leaf name).
/// Instrumentation stays inert unless measurement is requested.
pub(super) fn timing<T>(phase: &'static str, work: impl FnOnce() -> T) -> T {
    let _span = mojito_common::timing::span(phase);
    work()
}

/// One profile's complete optimization policy: the pliron cleanup stage plus
/// the profile's LLVM pipeline selection.
pub(super) struct Pipeline {
    profile: OptLevel,
}

impl Pipeline {
    pub(super) fn for_profile(profile: OptLevel) -> Pipeline {
        Pipeline { profile }
    }

    /// Run the profile-independent pliron cleanup stage: rebuild SSA out of
    /// the variable-slot allocas and drop the dead scaffolding. The pass
    /// construction must stay in sync with [`PLIRON_FUNCTION_PASSES`].
    pub(super) fn run_pliron_passes(
        context: &mut Context,
        module: ModuleOp,
    ) -> Result<(), PlironError> {
        let mut module_passes = OpPass::<ModuleOp, Passes>::default();
        let mut per_func = Passes::default();
        per_func.add_pass(OpPass::<
            pliron_llvm::ops::FuncOp,
            pliron::opts::mem2reg::Mem2RegPass,
        >::default());
        per_func
            .add_pass(OpPass::<pliron_llvm::ops::FuncOp, pliron::opts::dce::DCEPass>::default());
        module_passes.add_pass(NestedOpsPass::new(per_func));
        let mut analyses = AnalysisManager::default();
        // The instrumented lane: verify around every individual pass, not
        // just the whole pipeline (`compile` always verifies before and
        // after). Off unless requested — verification is quadratic-ish.
        if std::env::var_os("MOJITO_PLIRON_VERIFY_EACH_PASS").is_some() {
            analyses.set_config(pliron::pass::PMConfig {
                verify_before_all: true,
                verify_after_all: true,
                ..Default::default()
            });
        }
        module_passes
            .run(module.get_operation(), context, &mut analyses)
            .map_err(|error| PlironError {
                function: None,
                kind: PlironErrorKind::Emit(format!(
                    "cleanup pass pipeline failed: {}",
                    error.disp(context)
                )),
                location: None,
            })?;
        Ok(())
    }

    /// The LLVM pass pipeline the external `opt` runs over emitted bitcode,
    /// when the profile has one.
    pub(super) fn llvm_pipeline(&self) -> Option<&'static str> {
        match self.profile {
            OptLevel::O0 => None,
            OptLevel::Release => Some("default<O1>"),
        }
    }

    /// Stable rendering of the profile's full pipeline policy, pinned by the
    /// snapshot tests below so policy changes are always deliberate diffs.
    /// Test-only until the toolchain report surfaces it on the CLI.
    #[cfg(test)]
    pub(super) fn describe(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("profile: {}\n", self.profile_name()));
        out.push_str(&format!(
            "pliron function passes: {}\n",
            PLIRON_FUNCTION_PASSES.join(", ")
        ));
        match self.llvm_pipeline() {
            None => out.push_str("llvm opt pipeline: (none)\n"),
            Some(passes) => {
                out.push_str(&format!("llvm opt pipeline: {passes}\n"));
                out.push_str(&format!(
                    "opt argv template: opt -passes={passes} <bitcode> -o <bitcode>\n"
                ));
            }
        }
        out
    }

    #[cfg(test)]
    fn profile_name(&self) -> &'static str {
        match self.profile {
            OptLevel::O0 => "O0",
            OptLevel::Release => "release (alias: 1)",
        }
    }
}

/// The pliron cleanup stage's ordered per-function pass names, as rendered by
/// [`Pipeline::describe`]. Kept adjacent to [`Pipeline::run_pliron_passes`],
/// which constructs exactly this sequence.
#[cfg(test)]
const PLIRON_FUNCTION_PASSES: &[&str] = &["mem2reg", "dce"];

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use pliron::builtin::op_interfaces::SingleBlockRegionInterface;
    use pliron::context::Ptr;
    use pliron::irbuild::IRStatus;
    use pliron::linked_list::ContainsLinkedList;
    use pliron::operation::Operation;
    use pliron::pass::{Analysis, PassResult};

    /// Test analysis: the number of immediately nested operations.
    struct NestedOpCount {
        count: usize,
    }

    impl Analysis for NestedOpCount {
        fn name(&self) -> &str {
            "nested-op-count"
        }

        fn compute(
            op: Ptr<Operation>,
            ctx: &Context,
            _analyses: &mut AnalysisManager,
        ) -> pliron::result::Result<Self> {
            let mut count = 0;
            for region in op.deref(ctx).regions() {
                for block in region.deref(ctx).iter(ctx) {
                    count += block.deref(ctx).iter(ctx).count();
                }
            }
            Ok(NestedOpCount { count })
        }
    }

    /// Test pass: computes [`NestedOpCount`], optionally appends a nested
    /// module (a real mutation), and reports the configured status and
    /// preservation — the knobs the invalidation contract turns on.
    struct ProbePass {
        target: ModuleOp,
        mutate: bool,
        report_changed: bool,
        preserve: bool,
    }

    impl Pass for ProbePass {
        fn name(&self) -> &str {
            "probe"
        }

        fn run(
            &mut self,
            op: Ptr<Operation>,
            ctx: &mut Context,
            analyses: &mut AnalysisManager,
        ) -> pliron::result::Result<PassResult> {
            analyses.get_analysis::<NestedOpCount>(op, ctx)?;
            if self.mutate {
                let child = ModuleOp::new(ctx, "child".try_into().expect("valid identifier"));
                self.target.append_operation(ctx, child.get_operation(), 0);
            }
            let mut result = PassResult::default();
            result.ir_changed = if self.report_changed {
                IRStatus::Changed
            } else {
                IRStatus::Unchanged
            };
            if self.preserve {
                result.set_preserved::<NestedOpCount>();
            }
            Ok(result)
        }
    }

    #[test]
    fn pliron_analysis_evicted_when_mutating_pass_does_not_preserve() {
        let mut ctx = Context::new();
        let module = ModuleOp::new(&mut ctx, "m".try_into().expect("valid identifier"));
        let mut passes = Passes::default();
        passes.add_pass(ProbePass {
            target: module,
            mutate: true,
            report_changed: true,
            preserve: false,
        });
        let mut analyses = AnalysisManager::default();
        passes
            .run(module.get_operation(), &mut ctx, &mut analyses)
            .expect("pipeline runs");
        assert!(
            analyses
                .try_get_analysis::<NestedOpCount>(module.get_operation())
                .is_none(),
            "a mutating pass that does not preserve the analysis must evict it"
        );
    }

    #[test]
    fn pliron_analysis_retained_when_preserved_or_unchanged() {
        for (mutate, report_changed, preserve) in [(true, true, true), (false, false, false)] {
            let mut ctx = Context::new();
            let module = ModuleOp::new(&mut ctx, "m".try_into().expect("valid identifier"));
            let mut passes = Passes::default();
            passes.add_pass(ProbePass {
                target: module,
                mutate,
                report_changed,
                preserve,
            });
            let mut analyses = AnalysisManager::default();
            passes
                .run(module.get_operation(), &mut ctx, &mut analyses)
                .expect("pipeline runs");
            assert!(
                analyses
                    .try_get_analysis::<NestedOpCount>(module.get_operation())
                    .is_some(),
                "preserved-or-unchanged (mutate={mutate}) must retain the analysis"
            );
        }
    }

    #[test]
    fn pliron_wrongly_preserved_analysis_serves_stale_data() {
        // The failure mode the invalidation contract exists to prevent: a
        // pass that mutates but wrongly preserves leaves a cached analysis
        // that no longer matches a fresh compute.
        let mut ctx = Context::new();
        let module = ModuleOp::new(&mut ctx, "m".try_into().expect("valid identifier"));
        let mut passes = Passes::default();
        passes.add_pass(ProbePass {
            target: module,
            mutate: true,
            report_changed: true,
            preserve: true,
        });
        let mut analyses = AnalysisManager::default();
        passes
            .run(module.get_operation(), &mut ctx, &mut analyses)
            .expect("pipeline runs");
        let stale = analyses
            .try_get_analysis::<NestedOpCount>(module.get_operation())
            .expect("wrongly preserved analysis is still cached")
            .count;
        let fresh = NestedOpCount::compute(module.get_operation(), &ctx, &mut analyses)
            .expect("fresh compute")
            .count;
        assert_eq!(stale, 0);
        assert_eq!(fresh, 1, "the mutation added one nested op");
        assert_ne!(stale, fresh, "the cached analysis is stale");
    }

    #[test]
    fn pliron_nested_pass_change_evicts_module_level_analyses() {
        // The production pipeline's exact composition: a module-level
        // OpPass<ModuleOp, Passes> around a NestedOpsPass. A change reported
        // inside the nested boundary must evict analyses cached for the
        // module op too (retain_preserved evicts by analysis type across
        // every op key).
        let mut ctx = Context::new();
        let module = ModuleOp::new(&mut ctx, "m".try_into().expect("valid identifier"));
        let child = ModuleOp::new(&mut ctx, "child".try_into().expect("valid identifier"));
        module.append_operation(&mut ctx, child.get_operation(), 0);

        let mut analyses = AnalysisManager::default();
        analyses
            .get_analysis::<NestedOpCount>(module.get_operation(), &ctx)
            .expect("precompute module-level analysis");

        let mut module_passes = OpPass::<ModuleOp, Passes>::default();
        let mut per_child = Passes::default();
        per_child.add_pass(OpPass::<ModuleOp, ProbePass>::new(
            Default::default(),
            ProbePass {
                target: child,
                mutate: true,
                report_changed: true,
                preserve: false,
            },
        ));
        module_passes.add_pass(NestedOpsPass::new(per_child));
        module_passes
            .run(module.get_operation(), &mut ctx, &mut analyses)
            .expect("nested pipeline runs");
        assert!(
            analyses
                .try_get_analysis::<NestedOpCount>(module.get_operation())
                .is_none(),
            "a nested change must evict module-level analyses"
        );
    }

    #[test]
    fn pliron_pipeline_snapshot_o0() {
        expect![[r#"
            profile: O0
            pliron function passes: mem2reg, dce
            llvm opt pipeline: (none)
        "#]]
        .assert_eq(&Pipeline::for_profile(OptLevel::O0).describe());
    }

    #[test]
    fn pliron_pipeline_snapshot_release() {
        expect![[r#"
            profile: release (alias: 1)
            pliron function passes: mem2reg, dce
            llvm opt pipeline: default<O1>
            opt argv template: opt -passes=default<O1> <bitcode> -o <bitcode>
        "#]]
        .assert_eq(&Pipeline::for_profile(OptLevel::Release).describe());
    }
}
