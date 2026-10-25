//! `automatic-inline`: upstream's `AutomaticInline`
//! (`lib/Transforms/AutomaticInline.cpp`), which its first optimization
//! pipeline after elaboration runs over the concrete program.
//!
//! It walks the call graph's strongly connected components bottom-up. Each
//! function first takes the bodies of its callees, which arrive already
//! rewritten, then runs [`AUTOMATIC_INLINE_PIPELINE`], as upstream's
//! `completeFunctionProcessing` runs its nested function pipeline over
//! every function, inlined into or not. A callee is inlined when it lies
//! outside the caller's component (`canInlineCallee`), so a recursive
//! callee inlines once into a caller outside its cycle, and when
//! `shouldInlineCallee` admits it: an always-inline callee at any size, an
//! undecorated one whose [`inline_cost`] is below
//! [`AUTOMATIC_INLINE_SIZE_BOUND`], and never a `@no_inline` one
//! ([`Admission::Automatic`]). The splice is `inline-parametric`'s, over
//! callees with no binders left; what it keeps a call there, it keeps here.
//!
//! [`inline_cost`]: super::inline_parametric::inline_cost

use super::canonicalize::CANONICALIZE;
use super::cse::CSE;
use super::inline_parametric::{Admission, inline_calls};
use super::mem2reg::MEM_2_REG;
use super::pre_elaboration::{BodyPass, CallGraphPass, Callees, Cycles};
use super::simplify_cf::SIMPLIFY_CF;
use super::sroa::SROA;
use mojito_mir::mir::{MirDeclarations, MirFunction};

pub const AUTOMATIC_INLINE: CallGraphPass = CallGraphPass {
    name: "automatic-inline",
    follows: |_, _| true,
    cycles: Cycles::CallerComponent,
    run: automatic_inline,
};

/// The [`inline_cost`] from which an undecorated callee keeps its calls.
///
/// Upstream's `shouldInlineCallee` compares `getNumOperations`, every op
/// of the callee after its nested pipeline, against `getInlineThreshold`'s
/// 50 at `-O3`, its default level; `inline_cost` leaves out the three
/// operations every body holds there, as [`INLINE_SIZE_BOUND`] does for
/// `InlineParametric`'s 35.
///
/// [`inline_cost`]: super::inline_parametric::inline_cost
/// [`INLINE_SIZE_BOUND`]: super::inline_parametric::INLINE_SIZE_BOUND
pub const AUTOMATIC_INLINE_SIZE_BOUND: usize = 47;

/// The function pipeline upstream's `AutomaticInline` nests, in order, as
/// Mojito adopts it.
///
/// It is `SimplifyCF`, `SROA`, `Mem2Reg`, `Canonicalizer`, `CSEPass`,
/// `Canonicalizer`, `SimplifyCF`, `SROA`, `Mem2Reg`, `CSEPass`, and
/// `Canonicalizer`. Upstream's `HoistTrivialInvariants`, after the first
/// `CSEPass` (R1340), and `StackReuse`, before the second (R1341), are not
/// ported.
pub const AUTOMATIC_INLINE_PIPELINE: &[BodyPass] = &[
    SIMPLIFY_CF,
    SROA,
    MEM_2_REG,
    CANONICALIZE,
    CSE,
    CANONICALIZE,
    SIMPLIFY_CF,
    SROA,
    MEM_2_REG,
    CSE,
    CANONICALIZE,
];

/// Inline into `function` the callees [`AUTOMATIC_INLINE`] admits, then run
/// [`AUTOMATIC_INLINE_PIPELINE`] over it; returns whether either changed
/// it.
pub fn automatic_inline(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callees: &Callees<'_>,
) -> bool {
    let admission = Admission::Automatic {
        bound: AUTOMATIC_INLINE_SIZE_BOUND,
    };
    let inlined = inline_calls(name, function, declarations, callees, admission);
    AUTOMATIC_INLINE_PIPELINE
        .iter()
        .fold(inlined, |changed, pass| {
            (pass.run)(name, function, declarations) | changed
        })
}
