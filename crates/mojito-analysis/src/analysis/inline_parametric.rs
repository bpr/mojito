//! `inline-parametric`: upstream's `InlineParametric`, which its
//! pre-elaboration pipeline runs to open its second round, so that a call
//! to an `@always_inline("nodebug")` or `@always_inline("builtin")`
//! function, or to a plain `@always_inline` function small enough, holds
//! the callee's body instead of naming an instance of it.
//!
//! The callee's body is its finished one, drops already inserted, so its
//! drops come with it. Every register it brings takes the call's location,
//! which is what `nodebug` asks for: the inlined code steps as the call.
//! The pin inlines a `nodebug` or `builtin` callee at any size and a plain
//! `@always_inline` one below a size bound ([`INLINE_SIZE_BOUND`] over
//! [`inline_cost`]), and refuses one that lies on a cycle of such calls.
//! This pass also keeps the call to a callee holding a `try`, which the
//! pin inlines (R1288).
//!
//! A callee this pass inlines is a function, a method, a subscript's
//! accessor, which calls it on the subscripted base, a struct's one
//! `__init__`, which a call naming the struct calls, or a nested `def`,
//! which a call through a closure the caller makes calls, whose own
//! compile-time parameters the call binds in full; it takes its arguments
//! into `read`, owned, `mut`, or `ref` parameters, or into a `deinit`
//! one, which takes the slot the argument forwards or loads whole in
//! place, as a destructor takes the slot a drop of it forwards, or the
//! owned part an explicit destroy or a field's lowered drop loads and then
//! consumes, as upstream passes the field's address, or a fresh slot a
//! copied receiver's move or another argument initializes, and returns a
//! value or a reference. A part-bound body's places compose with the
//! part, and the caller's consume of it after the call is the body's end
//! of its receiver, on the raise path too, since the call's error region
//! follows it. A positional collector is a fresh slot the call's block
//! initializes from a `tuple.make` of the overflow arguments, a homogeneous
//! one's wrapped in its `VariadicList`, as upstream's call builds the pack
//! it passes; a spread binds the caller's
//! collector whole; the keyword collector the call passes as its `**`
//! operand, the dictionary it builds or a forwarded `**kwargs^`, binds the
//! callee's collector slot whole, as upstream's call passes it. The call's
//! positional and keyword arguments bind to the callee's parameters
//! through `mojito_ast::call`'s matcher, as the VM binds them, and an
//! omitted argument takes its default, materialized at
//! the call as upstream's parser materializes it in the caller: a literal,
//! or a converting constructor run on one. A default computed by a
//! function of its own keeps the call.
//! The call's compile-time arguments replace the callee's binders through
//! every type and parameter site of the body, as upstream substitutes them
//! through MLIR's `AttrTypeReplacer`; an argument over the caller's own
//! binders stays symbolic, for the elaborator to fold in each instance of
//! the caller. A parametric owner's binders take the arguments of the
//! receiver's type, or of the constructed one, which the member's bare
//! spelling of its owner denotes too; an owner keyed on one type pack
//! (`Tuple[*Ts]`) binds it to that type's element list. A read of a value binder, the
//! owner's (`Self.k`) included, becomes its argument as a constant,
//! upstream's `kgen.param.constant`. A variadic binder binds to its element
//! list, a spread of a type pack (`Tuple[*Ts]`) to its elements, and a pack
//! the call forwards (`f[*Us]()`) to the caller's own, and a struct keyed
//! on one pack applied to no element (`Tuple()`) binds it to the empty
//! list. A callable value binder (`f: def(Int) thin -> Int`) binds to the
//! function the call's register names, as upstream substitutes the
//! parameter value: the body's reads of its slot become that function as a
//! constant and a call through the slot a call through the constant. One
//! whose register names a closure of reference captures the caller makes
//! binds to that closure, as upstream substitutes the closure value: the
//! body's slot holds it, the inlined entry storing it there at the
//! closure's type, so a call through the slot is a call through a closure
//! the caller makes, and the call's accesses to the captured places are
//! that closure call's. One whose register names a closure with a copied
//! or moved capture, one loaded from a slot, or nothing keeps the call. A callable-bound type binder (`F: def(Int) -> Int`) binds as any
//! type. An origin argument binds nothing: the declaration's binders are
//! its type and value parameters, and the body's origin parameters stay
//! symbolic in the caller as in an instance. A binder's constraints, the
//! callee's `where` clauses, and its availability as a conditional member
//! bind nothing either: the checker judged them at the call, as the pin
//! judges a clause there (a call from a generic caller without the evidence
//! is rejected as "lacking evidence to prove correctness"), so the inlined
//! body carries none. A `comptime for` sequence closes over the bindings
//! as a lane mask does, its loop binder staying the body's own, which the
//! elaborator unrolls in the caller. A compile-time application over a
//! bound type pack (`Tuple.reverse`'s `comptime for` bound) is applied to
//! the bound list, which the elaborator evaluates in the caller's
//! instance. A construction of a bound type binder (`T()`, `Ts[0]()`,
//! `T(copy=x)`) is the bound type's own, as the elaborator writes it
//! (`mir::construct_type_parameters`). What still keeps the call: a value
//! only the elaborator folds (a deferred or marker value), a type that
//! still names a bound pack after substitution (a pack queried under a
//! loop binder), a construction the binding leaves open (`T()` at a
//! symbolic non-binder type, `Ts[i]()` under a loop binder, R445), and a
//! binding the body's sites cannot hold (a conformance test of a concrete
//! binding, R1096).
//!
//! An owned parameter, or a `read` one of a type no destructor tears down,
//! is a fresh slot the argument initializes, and so is a `read` one passed
//! a trivial copy of a slot of a closed type; a `mut` or `ref` parameter,
//! a `mut self` included, is the caller's place the call passes, and any
//! other `read` parameter the caller's place the call retained for it or
//! the argument was loaded from, directly or through a reference the same
//! block made (`self.copy()` reading `self` through a `ref.make`), as
//! upstream's inliner substitutes the argument's address for the callee's.
//! A callee returning a reference binds a `read` parameter to the place the
//! call retains for it too, so the reference its body makes is to the
//! caller's place; one returned to a fresh slot keeps the call. A
//! place-bound parameter the callee names as a place's root composes the
//! caller's place; one it copies whole loads the caller's place and copies
//! the value, naming the copy constructor its instantiated type has; one it
//! iterates, or lends an interior of, is the caller's slot; one it names
//! otherwise, or reads through a reference the block did not make, keeps
//! the call (R1099). A default is a fresh slot, dropped after
//! the body when the parameter only borrows it. An initializer's
//! `out self` is a fresh slot the body initializes, which the call's result
//! takes forwarded, with no move initializer, as upstream hands the value
//! to the call's user. A nested `def`'s captures, its leading parameters, are
//! the places the closure captures by reference, and a read of an
//! enclosing declaration's value binder is a reference to that binder, in
//! whose scope the caller lies. A copied or moved capture the caller's
//! frame holds as an environment slot of its own binds as a reference
//! capture of that slot; one a closure value holds keeps the call. A body
//! declared in a `comptime for` names the loop's binder, which the
//! caller's own scope or a `comptime for` holding the call declares, as
//! upstream inlines the generator inside the loop it unrolls; one called
//! outside such a loop, and a call through a closure the caller does not
//! make, keep the call. A call of any other shape stays a call.
//!
//! A raising callee's raises leave the inlined body as its returns do, as
//! upstream's inlined `lit.error_return` does: each `raise` breaks the
//! wrapper loop, whose results after the call's value are the call's error
//! flag and error, set by the raise and cleared by a return, and the
//! call's error region after the loop destroys what the caller holds and
//! raises on. The body's own error regions have already destroyed what it
//! holds, as the callee's frame does at the pin. A raise of a string
//! converts to `Error` as the raise does; a body raising any other value
//! of another type than the call's error keeps the call.
//!
//! Every body is placed as upstream's `inlineGeneratorCall` places it: in
//! a loop labelled [`LoopLabel::INLINED_CF_SCOPE`], its returns, at any
//! depth, breaks of that loop that store a returned value to the result
//! slot the call's register takes forwarded after it, with no move
//! initializer, as upstream's break carries the value. The caller-side
//! reads that bind an argument look back only over the straight-line run
//! lowering would leave the call in, which a region op ends.
//!
//! A drop of a slot whose struct's `__deinit__` is such a function holds the
//! destructor's body, as upstream inserts the destructor's call at the death
//! point and inlines it as any call: its `deinit self` is the dropped slot
//! itself, which the body's own consumption ends. A drop of a field holds
//! it too, its `deinit self` the field's place, which the caller's consume
//! after it ends: a synthesized destructor's fields, a `deinit self`'s
//! remaining ones, and a field a store overwrites, through a `mut self`
//! too. Only a drop whose slot holds the value whole on every path to it,
//! an `except` handler's entered from the body's raise points, is so
//! expanded (`mir::lower_whole_drops`), since a drop of any other tests at
//! run time what the slot still holds.

use super::pre_elaboration::{CallGraphPass, Callees};
use super::region_walk::region_op;
use mojito_ast::ast::{ArgConvention, Dtype, FORWARDED_KWARGS_NAME};
use mojito_ast::call::{ArgSlot, CallSlots, CallVariadics, bind_spread, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_checked::checked::ComptimeSequence;
use mojito_checked::declaration::{InlineLevel, SlotParameter};
use mojito_common::token::SourceSpan;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::{instruction_places, instruction_places_mut};
use mojito_mir::mir::visit::{
    MutTypeVisitor, ParamArgSite, ParamDeclSite, TySite, walk_function_mut,
};
use mojito_mir::mir::{
    Const, ConstructionTypes, FuncRef, LoopLabel, MirBlock, MirCaptureAccess, MirCaptureMode,
    MirClosureCapture, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirParamArg,
    MirPlace, MirSubscriptCall, MirTerm, Proj, Raises, Reg, RegionFrame, TypeConstructionError,
    UseMode, callable_targets, construct_type_parameters, count_register_reads,
    erase_removed_operands, fresh_register, has_no_destructor, instruction_regions,
    instruction_regions_mut, instruction_regs_mut, instruction_slots, instruction_subscript_call,
    instruction_subscript_call_mut, loop_breaks_mut, loop_continues_mut, lower_copies_and_moves,
    member_receiver_ty, name_constructors, region_exit, removed_argument_operands,
    renumber_block_slots, terminator_regs_mut, with_removed_argument_forwards_erased,
    without_removed_arguments,
};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{
    MetaTy, ParamBindings, ParamContext, ParamError, ParamExpr, ParamId, ParamKind, ParamRef,
};
use mojito_types::types::{
    GenericConstraint, LaneMask, ParamDecl, Ty, TyArg, TyKind, TyRewrite, VariantIndex,
    default_literal, is_scalar_simd, list_spread_argument, names_binder, pack_spread,
    pack_spread_argument, replace_argument_parameters, replace_constraint_parameters,
    replace_parameters, replace_value_parameters, rewrite_ty, scalar_simd_dtype,
};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

pub const INLINE_PARAMETRIC: CallGraphPass = CallGraphPass {
    name: "inline-parametric",
    follows: |declaration, _| is_inlined_before_elaboration(declaration),
    run: inline_parametric,
};

/// Whether upstream's `InlineParametric` reads the body of a function so
/// declared: the calls to one are the edges of the call graph the pass
/// walks.
///
/// It inlines an `@always_inline("nodebug")` or `@always_inline("builtin")`
/// callee before elaboration at any size, and a plain `@always_inline`
/// callee whose finished body's [`inline_cost`] is below
/// [`INLINE_SIZE_BOUND`].
pub const fn is_inlined_before_elaboration(declaration: &MirFunctionDeclaration) -> bool {
    matches!(
        declaration.inline,
        InlineLevel::NoDebug | InlineLevel::Builtin | InlineLevel::Always
    )
}

/// The [`inline_cost`] from which a plain `@always_inline` callee keeps its
/// calls: the pin inlines a body of 31 `s += x` statements and keeps one of
/// 32. Upstream's `shouldInline` compares `getNumOperations`, every op of
/// the generator, against `getInlineThreshold`'s 35 at `-O3`, and this
/// count leaves out three operations every body holds there.
pub const INLINE_SIZE_BOUND: usize = 32;

/// The size of `callee`'s finished body as the pin's bound measures it: a
/// count of the operations its own lowering would hold, calibrated against
/// the pin on probe bodies.
///
/// The pin counts the operations left after its first round, so what that
/// round erases is free: a slot's reads, stores, and declarations, which
/// its `Mem2Reg` promotes, a copy or drop that runs no constructor or
/// destructor, a `comptime assert`, and every constant but the first of
/// its value, which its canonicalizer folds into one. Each other
/// instruction is one operation. A place read or written through a field
/// costs one `struct.gep` more per projection, a read-modify-write's store
/// reusing its read's, and a raising body two more for its error result.
/// Control flow is priced as the structured ops the pin holds before it
/// lowers them: an `if` costs one, and one more per `elif`, as each
/// `branch` lowering makes of it, and each reached `continue` the four
/// operations its loop, its condition's test, and its break and continue
/// cost there beyond its own test and body. A `try` costs four beside its
/// parts. A `for` loop's `__iter__` call costs its body, which the pin
/// inlines before it measures; the `try` around its `__next__` call is
/// priced as any other. `print`, a builtin call here where the pin calls
/// the library's, costs the 30 the pin's `print(s)` call site holds.
pub fn inline_cost(callee: &MirFunction, callees: &Callees<'_>) -> usize {
    let mut constants = BTreeSet::new();
    blocks_cost(&callee.blocks, callees, &mut constants)
        + LOOP_COST * structured_back_edges(&callee.blocks)
        + if callee.raises { RAISES_COST } else { 0 }
}

/// Inline into `function` every call [`Callees::body`] answers whose shape
/// this pass inlines, in its `try` regions too; returns whether it inlined
/// one.
pub fn inline_parametric(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callees: &Callees<'_>,
) -> bool {
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.blocks, &mut reads);
    let function_values = callable_targets(function);
    let mut blocks = std::mem::take(&mut function.blocks);
    let closures = Closures::of(&blocks);
    let mut inliner = Inliner {
        function,
        declarations,
        callees,
        closures,
        function_values,
        caller: name,
        dropped_operands: false,
        loop_binders: Vec::new(),
    };
    let changed = inliner.block_list(&mut blocks);
    let dropped_operands = inliner.dropped_operands;
    function.blocks = blocks;
    if dropped_operands {
        erase_removed_operands(function, declarations, &reads);
    }
    if changed {
        name_constructors(function, declarations);
        super::name_destructors(function, declarations);
        lower_copies_and_moves(name, function, &|name| declarations.function(name));
        record_lowered_callees(function, callees);
    }
    changed
}

/// What a loop costs at the pin beyond its test and body: the pin keeps a
/// body of five `while s < x: s += x` loops and inlines one of four.
const LOOP_COST: usize = 4;

/// What raising costs a body at the pin: `pass` in a raising body holds
/// two operations more there than in a non-raising one.
const RAISES_COST: usize = 2;

/// What a `try` costs at the pin beyond its parts: an empty
/// `try: pass except: pass` holds four operations there.
const TRY_COST: usize = 4;

/// What a `print` call costs at the pin: `print(s)` inlines alone and keeps
/// the call with two `s += x` statements more.
const PRINT_COST: usize = 30;

/// The caller, its blocks taken out, and where its callees come from.
struct Inliner<'f, 'c> {
    function: &'f mut MirFunction,
    declarations: &'c MirDeclarations,
    callees: &'c Callees<'c>,
    /// The closures the caller makes, which a call through one calls.
    closures: Closures,
    /// The function each register of the caller statically holds, beside
    /// whether its value captures nothing: what a callable binder's
    /// argument register names.
    function_values: HashMap<u32, (String, bool)>,
    /// The caller's name: a member's body spells its own struct bare.
    caller: &'c str,
    /// Whether an inlined call passed an operand its inlined body does not
    /// take: one for a removed argument, or the run-time register of a
    /// compile-time argument, which the instantiated body reads as a
    /// value.
    dropped_operands: bool,
    /// The binders of the `comptime for` loops enclosing the block list
    /// being inlined into, outermost first.
    loop_binders: Vec<ParamId>,
}

/// Where an inlined call stands: its index in the caller's block, the
/// register it defines, and its raise edge.
struct CallSite {
    index: usize,
    dest: Reg,
    raises: Option<Raises>,
}

/// A call [`Inliner::inlinable`] inlines: the register it defines, its
/// callee instantiated at it, the binding of each parameter, whether it
/// passed an operand its inlined body does not take
/// ([`Inliner::dropped_operands`]), and each callable binder's
/// slot of the callee bound to a caller's closure register.
struct Inlinable<'c> {
    dest: Reg,
    callee: Cow<'c, MirFunction>,
    bindings: Vec<Binding>,
    dropped: bool,
    closures: Vec<(VarId, Reg)>,
}

/// A callee instantiated at a call: its body, and each callable binder's
/// slot of it bound to a caller's closure register.
struct Instantiated<'c> {
    body: Cow<'c, MirFunction>,
    closures: Vec<(VarId, Reg)>,
}

/// How an inlined parameter's slot is bound.
enum Binding {
    /// A fresh slot, initialized from the argument register.
    Value(Reg),
    /// The caller's place the argument was loaded from.
    Place(MirPlace),
    /// The caller's slot itself, forwarded to a `deinit` parameter: a
    /// destructor's `deinit self` at a drop of the slot, which upstream
    /// passes by address. The body ends the slot as the forward did.
    Slot(VarId),
    /// The caller's owned part an explicit destroy, or a field's drop,
    /// loads into a `deinit` parameter and consumes after the call, which
    /// upstream passes by address: the body's places compose with it, and
    /// the caller's consumption of it is the body's end of the parameter.
    Part { place: MirPlace, argument: Reg },
    /// A constructor's `out self`: a fresh slot the body initializes and
    /// the call's result takes forwarded.
    Out,
    /// A fresh slot, initialized from the omitted argument's default
    /// materialized at the call, and dropped after the body when the
    /// parameter only borrows it.
    Default { value: DefaultValue, dropped: bool },
}

/// The runtime arguments a call passes, positionally and by keyword, with
/// the caller's places each retains.
struct CallArguments<'a> {
    args: &'a [Reg],
    arg_places: &'a [Option<MirPlace>],
    kwargs: &'a [(String, Reg)],
    kwarg_places: &'a [Option<MirPlace>],
}

impl CallArguments<'_> {
    /// The argument each of `declaration`'s regular parameters past its
    /// first `captured`, a nested `def`'s captures, takes, and the
    /// arguments its collectors take, as the shared call matcher binds
    /// them; `None` when the call does not bind.
    fn slots(&self, declaration: &MirFunctionDeclaration, captured: usize) -> Option<CallSlots> {
        if declaration.required.len() != declaration.param_names.len() {
            return None;
        }
        let keywords: Vec<&str> = self.kwargs.iter().map(|(name, _)| name.as_str()).collect();
        let marker = |index: Option<usize>| {
            index.map_or(Some(None), |index| index.checked_sub(captured).map(Some))
        };
        match_call_slots(
            declaration.param_names.get(captured..)?,
            declaration.required.get(captured..)?,
            marker(declaration.positional_only)?,
            marker(declaration.keyword_only)?,
            self.args.len(),
            &keywords,
            CallVariadics {
                positional: declaration.variadic.is_some(),
                keyword: declaration.kw_variadic.is_some(),
            },
        )
        .ok()
    }
}

/// What a call binds its callee's parameters from: its receiver and the
/// receiver's place, a closure's captures, its matched arguments, and the
/// spread or forwarded collector it passes.
struct BindingSite<'a> {
    receiver: Option<Receiver>,
    receiver_place: Option<&'a MirPlace>,
    captures: &'a [MirClosureCapture],
    call: &'a CallArguments<'a>,
    matched: &'a CallSlots,
    /// The position of the spread argument, the positional collector's.
    spread: Option<usize>,
    /// The register of the forwarded `**kwargs^`, the keyword collector's.
    forwarded: Option<Reg>,
}

/// The closures a caller makes: each register a `closure.make` defines,
/// and each slot whose one store is such a register.
struct Closures {
    made: HashMap<u32, Closure>,
    slots: HashMap<VarId, u32>,
}

impl Closures {
    /// The closures `blocks`, a caller's body, makes, its regions included.
    fn of(blocks: &[MirBlock]) -> Self {
        let mut closures = Self {
            made: HashMap::new(),
            slots: HashMap::new(),
        };
        let mut stored = HashMap::new();
        closures.collect(blocks, &mut stored);
        closures.slots = stored
            .into_iter()
            .filter_map(|(var, src)| {
                src.filter(|src| closures.made.contains_key(src))
                    .map(|src| (var, src))
            })
            .collect();
        closures
    }

    /// Record the closures `blocks` makes, and each slot's store: its
    /// source register when it is stored once, `None` when more often.
    fn collect(&mut self, blocks: &[MirBlock], stored: &mut HashMap<VarId, Option<u32>>) {
        for instruction in blocks.iter().flat_map(|block| &block.instrs) {
            for region in instruction_regions(instruction) {
                self.collect(region, stored);
            }
            match instruction {
                MirInstr::MakeClosure {
                    dest,
                    function,
                    captures,
                } => {
                    self.made.insert(
                        dest.0,
                        Closure {
                            function: function.clone(),
                            captures: captures.clone(),
                        },
                    );
                }
                MirInstr::DefVar { var, src, .. } => {
                    stored
                        .entry(*var)
                        .and_modify(|source| *source = None)
                        .or_insert(Some(src.0));
                }
                _ => {}
            }
        }
    }

    /// The closure a call through `callee`, the value of the place `place`
    /// if it has one, calls: one the register holds, or the one its slot
    /// was stored once.
    fn called(&self, callee: Reg, place: Option<&MirPlace>) -> Option<&Closure> {
        self.made.get(&callee.0).or_else(|| {
            let place = place.filter(|place| place.is_whole_root() && place.through.is_none())?;
            self.made.get(self.slots.get(&place.root)?)
        })
    }
}

/// A closure a caller makes: the lifted nested `def` it calls, and the
/// caller's places it captures, which the lifted body takes as its leading
/// parameters.
struct Closure {
    function: String,
    captures: Vec<MirClosureCapture>,
}

/// An omitted argument's default, as the call materializes it.
enum DefaultValue {
    Literal(Literal),
    /// The converting constructor `target`, or the one `__init__` of the
    /// struct it names, run on a literal, at the parameter's type `ty`
    /// (`None` for an `Optional[T]`).
    Constructed {
        target: String,
        arg: Literal,
        ty: Ty,
    },
}

impl DefaultValue {
    /// `default` materialized at a parameter of type `ty`; `None` for a
    /// default this pass does not materialize.
    fn of(default: &CheckedConst, ty: &Ty, declarations: &MirDeclarations) -> Option<Self> {
        let CheckedConst::Construct { target, arg } = default else {
            return Literal::of(default, ty).map(Self::Literal);
        };
        let constructor = match declarations.structure(target) {
            Some(_) => declarations.function(&format!("{target}.__init__"))?,
            None => declarations.function(target)?,
        };
        let [parameter] = constructor.param_types.as_slice() else {
            return None;
        };
        if constructor.raises || constructor.ref_params.iter().any(|&reference| reference) {
            return None;
        }
        Some(Self::Constructed {
            target: target.clone(),
            arg: Literal::of(arg, parameter)?,
            ty: ty.clone(),
        })
    }

    /// Append to `instructions` the materialization of this value into
    /// fresh registers of `function` taking `span`; returns the register
    /// holding it.
    fn materialize(
        &self,
        function: &mut MirFunction,
        span: Option<&SourceSpan>,
        instructions: &mut Vec<MirInstr>,
    ) -> Reg {
        match self {
            Self::Literal(literal) => literal.materialize(function, span, instructions),
            Self::Constructed { target, arg, ty } => {
                let arg = arg.materialize(function, span, instructions);
                let dest = fresh_register(function, ty.clone(), span);
                instructions.push(MirInstr::Call {
                    dest,
                    func: FuncRef::named(target),
                    raises: None,
                    args: vec![arg],
                    kwargs: Vec::new(),
                    arg_places: vec![None],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    receiver: None,
                    instantiated_args: Vec::new(),
                    spread: None,
                });
                dest
            }
        }
    }
}

/// A literal default: its constant, the constant's type, and the scalar
/// type it materializes as, if it crosses the literal boundary.
struct Literal {
    k: Const,
    ty: Ty,
    materialized: Option<Ty>,
}

impl Literal {
    /// `default` read at a parameter of type `ty`, when it is a literal
    /// that type holds or a scalar materializes it as.
    fn of(default: &CheckedConst, ty: &Ty) -> Option<Self> {
        let scalar =
            matches!(ty.kind(), TyKind::Int | TyKind::UInt | TyKind::Float64) || is_scalar_simd(ty);
        let (k, own) = match (default, ty.kind()) {
            (CheckedConst::Int(value), TyKind::IntLiteral) => {
                (Const::IntLiteral(value.clone()), None)
            }
            (CheckedConst::Int(value), _) if scalar => {
                (Const::IntLiteral(value.clone()), Some(TyKind::IntLiteral))
            }
            (CheckedConst::Float(value), TyKind::FloatLiteral) => {
                (Const::FloatLiteral(value.clone()), None)
            }
            (CheckedConst::Float(value), _)
                if matches!(ty.kind(), TyKind::Float64)
                    || scalar_simd_dtype(ty).is_some_and(Dtype::is_float) =>
            {
                (
                    Const::FloatLiteral(value.clone()),
                    Some(TyKind::FloatLiteral),
                )
            }
            (CheckedConst::Bool(value), TyKind::Bool) => (Const::Bool(*value), None),
            (CheckedConst::String(value), TyKind::StringLiteral) => {
                (Const::Str(value.clone()), None)
            }
            (CheckedConst::Dtype(value), TyKind::Dtype) => (Const::Dtype(*value), None),
            (CheckedConst::None, TyKind::None) => (Const::None, None),
            _ => return None,
        };
        Some(match own {
            Some(own) => Self {
                k,
                ty: own.into(),
                materialized: Some(ty.clone()),
            },
            None => Self {
                k,
                ty: ty.clone(),
                materialized: None,
            },
        })
    }

    fn materialize(
        &self,
        function: &mut MirFunction,
        span: Option<&SourceSpan>,
        instructions: &mut Vec<MirInstr>,
    ) -> Reg {
        let dest = fresh_register(function, self.ty.clone(), span);
        instructions.push(MirInstr::Const {
            dest,
            k: self.k.clone(),
        });
        let Some(target) = &self.materialized else {
            return dest;
        };
        let value = dest;
        let dest = fresh_register(function, target.clone(), span);
        instructions.push(MirInstr::MaterializeLiteral {
            dest,
            value,
            target: target.clone(),
        });
        dest
    }
}

/// Where a call's callee takes its receiver from.
#[derive(Clone, Copy)]
enum Receiver {
    /// The call's first argument register, a method's `self`.
    Argument(Reg),
    /// The call's result, a constructor's `out self`.
    Constructed,
}

/// What a call binds its callee's owner's compile-time parameters to.
enum Owner<'c> {
    /// Nothing: the owner has none, or the call's type spells it bare, as
    /// a body of its own member does, whose binders are the callee's too.
    Unbound,
    Applied(OwnerInstance<'c>),
}

/// The parametric struct a callee is a member of, at the type a call
/// instantiates it.
struct OwnerInstance<'c> {
    /// The struct's name, which the member's body spells bare for its own
    /// type.
    template: String,
    /// The call's receiver or result type, an application of `template`.
    ty: Ty,
    decls: &'c [ParamDecl],
    /// The struct's fields, which a member read of the same spelling names
    /// rather than a binder.
    fields: &'c [(String, Ty)],
}

/// The caller register each register of an inlined callee becomes: past
/// the caller's.
struct Registers {
    base: u32,
}

impl Registers {
    /// The renumbering of an inlined callee's registers into `caller`'s.
    const fn new(caller: &MirFunction) -> Self {
        Self {
            base: caller.n_regs,
        }
    }

    const fn of(&self, reg: Reg) -> Reg {
        Reg(reg.0 + self.base)
    }
}

/// What an inlined call returns through.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Returned {
    /// Nothing: the callee returns `None`.
    None,
    /// The wrapper loop's result, which each `break` carries, as
    /// upstream's `hlcf.break` does: the call's register is that result,
    /// with no move initializer.
    Value,
    /// A constructor's `out self` slot, which the call's register takes
    /// forwarded.
    Out,
}

impl<'c> Inliner<'_, 'c> {
    /// Inline every inlinable call of `list`, a block list of the caller,
    /// in its regions too; returns whether it inlined one.
    ///
    /// A structured list holds one block, as does a structured callee's
    /// body; a call in a list of more, or of a callee whose body has more,
    /// stays a call.
    fn block_list(&mut self, list: &mut [MirBlock]) -> bool {
        let mut changed = false;
        let structured = list.len() == 1;
        for caller in list {
            let mut index = 0;
            while index < caller.instrs.len() {
                if !instruction_regions(&caller.instrs[index]).is_empty() {
                    changed |= self.instruction_regions(&mut caller.instrs[index]);
                }
                if structured
                    && let Some(Inlinable {
                        dest,
                        mut callee,
                        mut bindings,
                        dropped,
                        closures,
                    }) = self.inlinable(caller, index)
                    && callee.blocks.len() == 1
                    && let Some(blocks) =
                        self.inlined_blocks(callee.to_mut(), &bindings).or_else(|| {
                            demote_parts(&mut bindings)
                                .then(|| self.inlined_blocks(callee.to_mut(), &bindings))
                                .flatten()
                        })
                    && let raises = caller.instrs[index].raise_edge().cloned()
                    && raises.as_ref().is_none_or(|raises| {
                        routes_raises(&blocks, raises, &callee.reg_types, self.function.n_regs)
                    })
                {
                    self.dropped_operands |=
                        dropped || passes_value_arguments(&caller.instrs[index]);
                    changed = true;
                    // The body's loop is scanned next, its regions as any
                    // region op's.
                    index = self.splice(
                        caller,
                        &CallSite {
                            index,
                            dest,
                            raises,
                        },
                        &callee,
                        (&bindings, &closures),
                        blocks,
                    );
                    continue;
                }
                index += 1;
            }
        }
        changed
    }

    /// The declaration of `name`, when every binder of the regions it, and
    /// each declaration it is nested in, is declared in is in scope
    /// everywhere in the call being inlined: a binder of the caller's own
    /// scope or of a `comptime for` region enclosing the block list being
    /// inlined into. A body declared in a `comptime for` inlines only
    /// inside that loop, whose binder the inlined body names itself.
    fn scoped_declaration(&self, name: &str) -> Option<&'c MirFunctionDeclaration> {
        let declaration = self.declarations.function(name)?;
        let mut caller_scope: Vec<&ParamId> = Vec::new();
        let mut link = Some(self.caller);
        while let Some(current) = link {
            let scope = self.declarations.function(current);
            if let Some(scope) = scope {
                caller_scope.extend(
                    scope
                        .param_decls
                        .iter()
                        .chain(&scope.region_binders)
                        .map(ParamDecl::id),
                );
            }
            link = scope.and_then(|scope| scope.enclosing.as_deref());
        }
        let mut link = Some(declaration);
        while let Some(current) = link {
            if !current
                .region_binders
                .iter()
                .map(ParamDecl::id)
                .all(|id| caller_scope.contains(&id) || self.loop_binders.contains(id))
            {
                return None;
            }
            link = current
                .enclosing
                .as_deref()
                .and_then(|name| self.declarations.function(name));
        }
        Some(declaration)
    }

    /// Inline into the regions `instruction` holds; returns whether it
    /// inlined a call.
    fn instruction_regions(&mut self, instruction: &mut MirInstr) -> bool {
        if let MirInstr::ComptimeFor {
            binder,
            body,
            orelse,
            ..
        } = instruction
        {
            self.loop_binders.push(binder.id.clone());
            let changed = self.block_list(body);
            self.loop_binders.pop();
            return self.block_list(orelse) | changed;
        }
        instruction_regions_mut(instruction)
            .into_iter()
            .fold(false, |changed, region| self.block_list(region) | changed)
    }

    /// The generator of `full`, so declared, without its `removed`
    /// arguments: the body a direct call inlines. Its own forwards to its
    /// callees' removed arguments went before its arguments were judged, so
    /// it is read through the view the elaborator reads an instance
    /// through, the forwards erased and the operands kept for the
    /// elaborator to drop at each call.
    fn reduced_generator(
        &self,
        full: &MirFunction,
        full_declaration: &MirFunctionDeclaration,
        removed: &[VarId],
    ) -> (MirFunction, MirFunctionDeclaration) {
        let forwards = with_removed_argument_forwards_erased(full, self.declarations, &|callee| {
            full.callee_removed_args
                .iter()
                .find(|(discounted, _)| discounted == callee)
                .map(|(_, slots)| slots.as_slice())
        });
        let source = forwards.as_ref().map_or(full, |(view, _)| view);
        without_removed_arguments(source, full_declaration, removed)
    }

    /// The callee of the call at `index` of `block`, its compile-time
    /// parameters and its owner's bound to the call's, and the binding of
    /// each of its parameters, when the pass inlines the call.
    ///
    /// A call naming an initializer, or a struct (a call of its `__init__`),
    /// constructs: the initializer's `out self` is the call's result. A
    /// nested `def`'s direct call binds its leading capture parameters to
    /// the captured places it passes; a call through a closure the caller
    /// makes is a call of the nested `def` it lifts, whose captures bind
    /// the places the closure captures.
    fn inlinable(&self, block: &MirBlock, index: usize) -> Option<Inlinable<'c>> {
        let index_places = match &block.instrs[index] {
            MirInstr::Index { index_place, .. } => vec![index_place.as_deref().cloned()],
            _ => Vec::new(),
        };
        let (
            dest,
            target,
            call,
            compile_time,
            param_arg_regs,
            receiver,
            receiver_place,
            captures,
            spread,
            capture_accesses,
        ) = match &block.instrs[index] {
            MirInstr::Call {
                dest,
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                receiver: None,
                instantiated_args,
                spread,
                ..
            } => {
                let struct_named = self.declarations.structure(&func.0).is_some();
                let target = if struct_named {
                    Cow::Owned(format!("{}.__init__", func.0))
                } else {
                    Cow::Borrowed(func.0.as_str())
                };
                let constructed =
                    struct_named || mojito_symbol::symbol::is_initializer_symbol(&func.0);
                let receiver = constructed.then_some(Receiver::Constructed);
                (
                    *dest,
                    target,
                    CallArguments {
                        args,
                        arg_places,
                        kwargs,
                        kwarg_places,
                    },
                    Cow::Borrowed(instantiated_args.as_slice()),
                    param_arg_regs.as_slice(),
                    receiver,
                    None,
                    &[][..],
                    *spread,
                    capture_accesses.as_slice(),
                )
            }
            MirInstr::MethodCall {
                dest,
                recv,
                resolved: Some(target),
                result_adapter: None,
                args,
                kwargs,
                recv_place,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                instantiated_args,
                spread,
                ..
            } if capture_accesses.is_empty() => (
                *dest,
                Cow::Borrowed(target.as_str()),
                CallArguments {
                    args,
                    arg_places,
                    kwargs,
                    kwarg_places,
                },
                Cow::Borrowed(instantiated_args.as_slice()),
                param_arg_regs.as_slice(),
                Some(Receiver::Argument(*recv)),
                recv_place.as_deref(),
                &[][..],
                *spread,
                &[][..],
            ),
            // A checker-selected subscript calls its accessor on the
            // base, passing the index as the accessor's one runtime
            // argument or, to one keyed on a value parameter of its own
            // (`Tuple.__getitem_param__[index: Int]`), as that
            // parameter's constant, as the VM and the elaborator pass it.
            MirInstr::Index {
                dest,
                base,
                index: operand,
                base_place,
                call: Some(subscript),
                intrinsic: None,
                ..
            } if subscript.capture_accesses.is_empty() => {
                let parameterless = subscript.arguments.is_empty();
                let (args, arg_places) = if parameterless {
                    (&[][..], &[][..])
                } else {
                    (std::slice::from_ref(operand), index_places.as_slice())
                };
                (
                    *dest,
                    Cow::Borrowed(subscript.target.as_str()),
                    CallArguments {
                        args,
                        arg_places,
                        kwargs: &[],
                        kwarg_places: &[],
                    },
                    Cow::Owned(subscript_parameter_arguments(block, index, subscript)?),
                    &[][..],
                    Some(Receiver::Argument(*base)),
                    base_place.as_deref(),
                    &[][..],
                    None,
                    &[][..],
                )
            }
            MirInstr::CallIndirect {
                dest,
                callee,
                args,
                kwargs,
                callee_place,
                arg_places,
                kwarg_places,
                param_arg_regs,
                instantiated_args,
                spread,
                ..
            } => {
                let closure = self.closures.called(*callee, callee_place.as_deref())?;
                (
                    *dest,
                    Cow::Borrowed(closure.function.as_str()),
                    CallArguments {
                        args,
                        arg_places,
                        kwargs,
                        kwarg_places,
                    },
                    Cow::Borrowed(instantiated_args.as_slice()),
                    param_arg_regs.as_slice(),
                    None,
                    None,
                    closure.captures.as_slice(),
                    *spread,
                    &[][..],
                )
            }
            _ => return None,
        };
        // A forwarded `**kwargs^` is the keyword collector's one argument,
        // which the matcher does not see.
        let forwarded_at = call
            .kwargs
            .iter()
            .position(|(name, _)| name == FORWARDED_KWARGS_NAME);
        let forwarded = forwarded_at.map(|at| call.kwargs[at].1);
        let keyword_indexes: Vec<usize> = (0..call.kwargs.len())
            .filter(|&at| Some(at) != forwarded_at)
            .collect();
        let full = self.callees.body(&target)?;
        let full_declaration = self.scoped_declaration(&target)?;
        // A direct call names the generator without its removed arguments,
        // as upstream's `RemoveUnusedParams` rewrote it, and passes no
        // operand for them; that generator is the body inlined.
        let removed = match receiver {
            Some(Receiver::Constructed) => &[][..],
            _ if !captures.is_empty() => &[][..],
            _ => full.removed_args.as_slice(),
        };
        let (reduced, declaration, positional, keyword) = if removed.is_empty() {
            (
                None,
                Cow::Borrowed(full_declaration),
                BTreeSet::new(),
                BTreeSet::new(),
            )
        } else {
            let keywords: Vec<&str> = keyword_indexes
                .iter()
                .map(|&at| call.kwargs[at].0.as_str())
                .collect();
            let (positional, keyword) =
                removed_argument_operands(full_declaration, removed, call.args.len(), &keywords)
                    .ok()?;
            let (body, declaration) = self.reduced_generator(full, full_declaration, removed);
            (Some(body), Cow::Owned(declaration), positional, keyword)
        };
        let callee = reduced.as_ref().unwrap_or(full);
        let declaration = declaration.as_ref();
        if declaration.inline == InlineLevel::Always
            && inline_cost(callee, self.callees) >= INLINE_SIZE_BOUND
        {
            return None;
        }
        let kept = |indexes: &BTreeSet<usize>, length: usize| {
            (0..length)
                .filter(|index| !indexes.contains(index))
                .collect::<Vec<_>>()
        };
        let args: Vec<Reg> = kept(&positional, call.args.len())
            .into_iter()
            .map(|index| call.args[index])
            .collect();
        let arg_places: Vec<Option<MirPlace>> = kept(&positional, call.arg_places.len())
            .into_iter()
            .map(|index| call.arg_places[index].clone())
            .collect();
        let kwargs: Vec<(String, Reg)> = kept(&keyword, keyword_indexes.len())
            .into_iter()
            .map(|index| call.kwargs[keyword_indexes[index]].clone())
            .collect();
        let kwarg_places: Vec<Option<MirPlace>> = kept(&keyword, keyword_indexes.len())
            .into_iter()
            .filter_map(|index| call.kwarg_places.get(keyword_indexes[index]).cloned())
            .collect();
        let mut dropped = !positional.is_empty() || !keyword.is_empty();
        let (spread, forwarded, removed_collector) =
            collector_operands(spread, forwarded, declaration, full_declaration, args.len())?;
        dropped |= removed_collector;
        let call = CallArguments {
            args: &args,
            arg_places: &arg_places,
            kwargs: &kwargs,
            kwarg_places: &kwarg_places,
        };
        let receives = match receiver {
            Some(Receiver::Argument(_)) if !declaration.has_receiver => {
                dropped = true;
                false
            }
            other => other.is_some(),
        };
        let owner_ty = match receiver {
            Some(Receiver::Constructed) => {
                if declaration.receiver_convention != Some(ArgConvention::Out) {
                    return None;
                }
                Some(self.function.reg_types.get(&dest.0)?)
            }
            Some(Receiver::Argument(recv)) => Some(self.function.reg_types.get(&recv.0)?),
            None => None,
        };
        if !is_closed(callee, declaration)
            || declaration.source_arity() + usize::from(receives) != callee.n_params
        {
            return None;
        }
        let matched = call.slots(declaration, captures.len())?;
        // A keyword collector binds only the `**` operand the call passes,
        // the dictionary it builds or forwards, which is its whole
        // argument.
        if !matched.keyword_overflow.is_empty()
            || declaration.kw_variadic.is_some() != forwarded.is_some()
        {
            return None;
        }
        if let Some(position) = spread {
            bind_spread(&matched, position).ok()?;
        }
        let owner = match owner_ty.map(|ty| self.owner(full, ty)) {
            Some(Some(Owner::Applied(owner))) => Some(owner),
            Some(Some(Owner::Unbound)) | None => None,
            Some(None) => return None,
        };
        let Instantiated {
            body: mut callee,
            closures,
        } = self.instantiated_callee(
            (block, index),
            (param_arg_regs, &compile_time, capture_accesses),
            (full, reduced.as_ref(), declaration),
            owner.as_ref(),
        )?;
        if declaration.enclosing.is_some() {
            read_enclosing_binders(callee.to_mut(), declaration, self.declarations)?;
        }
        let bindings = self.bindings(
            block,
            index,
            &callee,
            declaration,
            &BindingSite {
                receiver,
                receiver_place,
                captures,
                call: &call,
                matched: &matched,
                spread,
                forwarded,
            },
        )?;
        if callee.returns_reference && !returns_bound_place(&callee, &bindings) {
            return None;
        }
        Some(Inlinable {
            dest,
            callee,
            bindings,
            dropped,
            closures,
        })
    }

    /// The callee of the call at `index` of `block`, its full body or the
    /// `reduced` generator, instantiated at the call's compile-time
    /// arguments, each callable one bound to what its register in
    /// `param_arg_regs` holds; `None` when they do not bind, or when the
    /// call's `capture_accesses` are to places neither a nested callee's
    /// captures nor a bound closure's.
    fn instantiated_callee(
        &self,
        (block, index): (&MirBlock, usize),
        (param_arg_regs, compile_time, capture_accesses): (
            &[MirParamArg],
            &[TyArg],
            &[MirCaptureAccess],
        ),
        (full, reduced, declaration): (
            &'c MirFunction,
            Option<&MirFunction>,
            &MirFunctionDeclaration,
        ),
        owner: Option<&OwnerInstance<'_>>,
    ) -> Option<Instantiated<'c>> {
        let callables: Vec<Option<CallableArgument>> = param_arg_regs
            .iter()
            .map(|argument| self.callable_argument(block, index, argument.value?))
            .collect();
        let arguments = Arguments {
            arguments: compile_time,
            callables: &callables,
        };
        let instantiated = match reduced {
            Some(body) => {
                let Instantiated { body, closures } =
                    instantiated(body, declaration, owner, &arguments, self.declarations)?;
                Instantiated {
                    body: Cow::Owned(body.into_owned()),
                    closures,
                }
            }
            None => instantiated(full, declaration, owner, &arguments, self.declarations)?,
        };
        // A nested body's direct call carries the body's own accesses to
        // the captured places, which the inlined body performs itself.
        (capture_accesses.is_empty()
            || declaration.enclosing.is_some()
            || accessed_through(capture_accesses, &instantiated.closures, &self.closures))
        .then_some(instantiated)
    }

    /// The binding of each of `callee`'s parameters at the call at `index`
    /// of `block`, in the body's slot layout (`slot_parameter`): the
    /// receiver, a nested `def`'s captures, the regular parameters from the
    /// matched slots, the positional collector from the overflow or the
    /// spread, and the keyword collector from the forwarded dictionary.
    fn bindings(
        &self,
        block: &MirBlock,
        index: usize,
        callee: &MirFunction,
        declaration: &MirFunctionDeclaration,
        site: &BindingSite<'_>,
    ) -> Option<Vec<Binding>> {
        let mut bindings = Vec::with_capacity(callee.n_params);
        for param in 0..callee.n_params {
            // A `read` argument with no place to bind, an owned temporary
            // such as a call's result, is a value when its type's drop runs
            // no code, as upstream substitutes a register-passable SSA value.
            let bind = |argument: Reg, place: Option<&MirPlace>| {
                argument_binding(self.function, callee, block, index, param, argument, place)
                    .or_else(|| {
                        (!is_reference_parameter(callee, param)
                            && callee
                                .param_types
                                .get(param)
                                .is_some_and(|ty| self.declarations.drop_runs_no_code(ty)))
                        .then_some(Binding::Value(argument))
                    })
            };
            let binding = match declaration.slot_parameter(param) {
                SlotParameter::Receiver => match site.receiver? {
                    Receiver::Constructed => Binding::Out,
                    Receiver::Argument(recv) => bind(
                        recv,
                        site.receiver_place
                            .filter(|_| is_reference_parameter(callee, param)),
                    )?,
                },
                SlotParameter::Position(position) if position < site.captures.len() => {
                    let capture = &site.captures[position];
                    if capture.mode != MirCaptureMode::Reference
                        || !is_reference_parameter(callee, param)
                    {
                        return None;
                    }
                    Binding::Place(capture.place.clone())
                }
                SlotParameter::Position(position) => {
                    match site.matched.slots.get(position - site.captures.len())? {
                        ArgSlot::Positional(source) => bind(
                            site.call.args[*source],
                            site.call.arg_places.get(*source).and_then(Option::as_ref),
                        )?,
                        ArgSlot::Keyword(source) => bind(
                            site.call.kwargs[*source].1,
                            site.call.kwarg_places.get(*source).and_then(Option::as_ref),
                        )?,
                        ArgSlot::Default => {
                            let default = declaration.defaults.get(position)?.as_ref()?;
                            default_binding(callee, param, default, self.declarations)?
                        }
                    }
                }
                // The collector the call builds is the slot's value, its
                // elements a `read` collector's borrows; a spread passes
                // the caller's collector whole, as any argument.
                SlotParameter::Variadic => {
                    let position = site.spread?;
                    let collector = site.call.args[position];
                    if builds_collector(block, index, collector) {
                        Binding::Value(collector)
                    } else {
                        bind(
                            collector,
                            site.call.arg_places.get(position).and_then(Option::as_ref),
                        )?
                    }
                }
                SlotParameter::KwVariadic => Binding::Value(site.forwarded?),
            };
            bindings.push(binding);
        }
        Some(bindings)
    }

    /// What the register `argument`, a call's compile-time argument passed
    /// at `index` of `block`, holds: the capture-free function the
    /// caller's dataflow resolves it to or the block defines it as a
    /// constant, or a closure of reference captures the caller makes;
    /// `None` for a closure with a copied or moved capture, one the caller
    /// loads from a slot, or an unknown value, whose call stays.
    fn callable_argument(
        &self,
        block: &MirBlock,
        index: usize,
        argument: Reg,
    ) -> Option<CallableArgument> {
        if let Some(closure) = self.closures.made.get(&argument.0) {
            return closure
                .captures
                .iter()
                .all(|capture| capture.mode == MirCaptureMode::Reference)
                .then_some(CallableArgument::Closure(argument));
        }
        match self.function_values.get(&argument.0) {
            Some((function, true)) => Some(CallableArgument::Function(function.clone())),
            Some((_, false)) => None,
            None => {
                match defined_constant(&block.instrs[run_start(block, index)..index], argument)? {
                    Const::Function(function) => Some(CallableArgument::Function(function.clone())),
                    _ => None,
                }
            }
        }
    }

    /// The struct whose method `callee` is, instantiated at `ty`, the type
    /// of the call's receiver or result; `None` when `ty` is no application
    /// of it.
    fn owner(&self, callee: &MirFunction, ty: &Ty) -> Option<Owner<'c>> {
        let TyKind::Struct(template, _) = callee.param_types.first()?.kind() else {
            return None;
        };
        let structure = self.declarations.structure(template)?;
        let decls = &structure.param_decls;
        if decls.is_empty() {
            return Some(Owner::Unbound);
        }
        match ty.kind() {
            // A bare spelling in a body of the struct's own member denotes
            // its own type, whose binders are the callee's too; anywhere
            // else it is the empty application of a struct keyed on one
            // pack (`Tuple()`), whose pack binds to no element.
            TyKind::Struct(name, arguments)
                if name == template
                    && arguments.is_empty()
                    && (self.is_member_of(template)
                        || !matches!(
                            decls.as_slice(),
                            [ParamDecl::Type { variadic: true, .. }]
                        )) =>
            {
                Some(Owner::Unbound)
            }
            TyKind::Struct(name, _) if name == template => Some(Owner::Applied(OwnerInstance {
                template: template.clone(),
                ty: ty.clone(),
                decls,
                fields: &structure.fields,
            })),
            _ => None,
        }
    }

    /// Whether the caller is `template` or one of its members.
    fn is_member_of(&self, template: &str) -> bool {
        self.caller == template
            || mojito_symbol::symbol::split_method_symbol(self.caller)
                .is_some_and(|(owner, _)| owner == template)
    }

    /// The callee's blocks, their regions included, with its slots and
    /// registers moved past the caller's and each place-bound parameter
    /// replaced by the caller's
    /// place, or `None` when a place-bound parameter is named other than as
    /// a place's root or as the whole value a copy or an iteration reads,
    /// or a place reached through a reference other than the parameter's
    /// composes with a caller's place reached through one. A copy of a
    /// place-bound parameter takes a fresh register of `callee`.
    fn inlined_blocks(
        &self,
        callee: &mut MirFunction,
        bindings: &[Binding],
    ) -> Option<Vec<MirBlock>> {
        let base = self.function.n_vars as VarId;
        let mut blocks = callee.blocks.clone();
        renumber_block_slots(&mut blocks, &|var| match bindings.get(var as usize) {
            Some(Binding::Slot(slot)) => *slot,
            _ => var + base,
        });
        let bound = |var: VarId| match bindings.get(var.checked_sub(base)? as usize)? {
            Binding::Place(place) | Binding::Part { place, .. } => Some(place),
            Binding::Value(_) | Binding::Default { .. } | Binding::Out | Binding::Slot(_) => None,
        };
        // The caller's consumption of a part-bound parameter's place after
        // the call is the body's end of it.
        let part_bound = |var: VarId| {
            var.checked_sub(base)
                .and_then(|param| bindings.get(param as usize))
                .is_some_and(|binding| matches!(binding, Binding::Part { .. }))
        };
        retain_unconsumed(&mut blocks, &part_bound);
        copy_bound_parameters(&mut blocks, callee, &bound);
        rebind_blocks(&mut blocks, &Registers::new(self.function), &bound)?;
        Some(blocks)
    }

    /// Replace the call at `site` of `caller`, the call's block, by
    /// `blocks`, the callee's body [`Self::inlined_blocks`] prepared: the
    /// block binds the parameters, then holds the body as [`splice_region`]
    /// places it, and resumes with what followed the call. Returns the
    /// position of the body's loop in the block.
    fn splice(
        &mut self,
        caller: &mut MirBlock,
        site: &CallSite,
        callee: &MirFunction,
        (bindings, closures): (&[Binding], &[(VarId, Reg)]),
        mut blocks: Vec<MirBlock>,
    ) -> usize {
        let declarations = self.declarations;
        let function = &mut *self.function;
        merge_callee_removals(
            &mut function.callee_removed_params,
            &callee.callee_removed_params,
        );
        let CallSite {
            index,
            dest,
            ref raises,
        } = *site;
        let mut after = caller.instrs.split_off(index + 1);
        caller.instrs.truncate(index);
        // A slot the body takes in place keeps its value until the body
        // ends it: the forward that fed the call goes, and so does the
        // caller's consume of a loaded receiver's residue after the call,
        // which the body's own consume of its receiver is.
        let run = run_start(caller, index);
        let resumed_run = after.iter().position(splits_runs).unwrap_or(after.len());
        for binding in bindings {
            let Binding::Slot(slot) = binding else {
                continue;
            };
            if let Some(forward) = caller.instrs[run..].iter().rposition(|instruction| {
                matches!(instruction, MirInstr::UseVar { var, mode: UseMode::Forward, .. } if var == slot)
            }) {
                caller.instrs.remove(run + forward);
            } else if let Some(consume) = after[..resumed_run]
                .iter()
                .position(|instruction| instruction_slots(instruction).contains(slot))
                && matches!(after[consume], MirInstr::ConsumeVar { var } if var == *slot)
            {
                after.remove(consume);
            }
        }
        let var_base = function.n_vars as VarId;
        let registers = Registers::new(function);
        let result = if matches!(bindings.first(), Some(Binding::Out)) {
            Returned::Out
        } else {
            result_of(callee)
        };
        let exit = ReturnExit {
            result,
            dest,
            raises: raises.clone(),
        };

        let call_span = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
        for reg in 0..callee.n_regs {
            let inlined = registers.of(Reg(reg));
            if let Some(ty) = callee.reg_types.get(&reg) {
                function.reg_types.insert(inlined.0, ty.clone());
            }
            if let Some(span) = &call_span {
                function.spans.0.insert(inlined.0, (span.clone(), None));
            }
        }
        function.n_regs += callee.n_regs;
        // A reference the body's breaks carry is the call's, of the
        // caller's reference type, and so is one a break of a loop it
        // returns the result of carries; the callee's own spells its origin
        // untracked.
        if callee.returns_reference
            && result == Returned::Value
            && let Some(ty) = function.reg_types.get(&dest.0).cloned()
        {
            let mut returned = Vec::new();
            references_and_returns(&blocks, &mut HashMap::new(), &mut returned);
            carried_by_breaks(&mut blocks, &mut returned);
            for reg in returned {
                function.reg_types.insert(reg.0, ty.clone());
            }
        }
        // A part the body takes in place ends at the caller's consume of it
        // after the call, whose marker the call's receiver load no longer
        // feeds.
        if bindings
            .iter()
            .any(|binding| matches!(binding, Binding::Part { .. }))
            && let Some(MirInstr::ConsumePlace { marker, .. }) = after.first_mut()
        {
            *marker = fresh_register(function, Ty::none(), call_span.as_ref());
        }
        for (var, name) in callee.var_names.iter().enumerate() {
            function.var_names.push(format!("{name}$inline{var_base}"));
            if let Some(ty) = callee.var_tys.get(&(var as VarId)) {
                function.var_tys.insert(var as VarId + var_base, ty.clone());
            }
        }
        function.n_vars += callee.n_vars;
        let mut resumed = Vec::with_capacity(after.len() + 1);
        match result {
            Returned::None => resumed.push(MirInstr::Const {
                dest,
                k: Const::None,
            }),
            // The initialized `out self` is the call's result on the normal
            // path alone: past the call's error region.
            Returned::Out => {
                let forward = MirInstr::UseVar {
                    dest,
                    var: var_base,
                    mode: UseMode::Forward,
                    constructor: None,
                };
                let edge = raises.as_ref().and_then(|raises| {
                    after.iter().position(|instruction| {
                        matches!(instruction, MirInstr::If { cond, .. } if *cond == raises.flag)
                    })
                });
                match edge {
                    Some(edge) => after.insert(edge + 1, forward),
                    None => resumed.push(forward),
                }
            }
            Returned::Value => {}
        }
        resumed.extend(bindings.iter().enumerate().filter_map(|(param, binding)| {
            let var = param as VarId + var_base;
            matches!(binding, Binding::Default { dropped: true, .. }).then(|| MirInstr::DropVar {
                var,
                destructor: function
                    .var_tys
                    .get(&var)
                    .and_then(|ty| declarations.destructor(ty))
                    .map(str::to_owned),
            })
        }));
        resumed.extend(after.into_iter().map(|instruction| match instruction {
            MirInstr::DefVar {
                var,
                src,
                binding_ty,
                in_place: true,
            } if src == dest => MirInstr::DefVar {
                var,
                src,
                binding_ty,
                in_place: false,
            },
            other => other,
        }));

        for (param, binding) in bindings.iter().enumerate() {
            let var = param as VarId;
            let ty = callee
                .var_tys
                .get(&var)
                .or_else(|| callee.param_types.get(param))
                .cloned();
            match binding {
                Binding::Value(argument) => caller.instrs.push(MirInstr::DefVar {
                    var: var + var_base,
                    src: *argument,
                    binding_ty: ty,
                    in_place: false,
                }),
                Binding::Default { value, .. } => {
                    let src = value.materialize(function, call_span.as_ref(), &mut caller.instrs);
                    caller.instrs.push(MirInstr::DefVar {
                        var: var + var_base,
                        src,
                        binding_ty: ty,
                        in_place: false,
                    });
                }
                Binding::Out => caller.instrs.push(MirInstr::MarkInitialized {
                    place: MirPlace::root(var + var_base, ty),
                }),
                Binding::Place(_) | Binding::Slot(_) | Binding::Part { .. } => {}
            }
        }
        // A callable bound to a caller's closure holds it, at the closure's
        // type, as upstream substitutes the closure value for the
        // parameter: the slot and each read of it take that type.
        for &(slot, closure) in closures {
            let var = slot + var_base;
            let ty = function.reg_types.get(&closure.0).cloned();
            if let Some(ty) = &ty {
                function.var_tys.insert(var, ty.clone());
                retype_closure_slot(&mut blocks, var, ty, &mut function.reg_types);
            }
            caller.instrs.push(MirInstr::DefVar {
                var,
                src: closure,
                binding_ty: ty,
                in_place: false,
            });
            self.closures.slots.insert(var, closure.0);
        }
        splice_region(function, caller, blocks, resumed, &exit)
    }
}

/// Place `blocks`, an inlined body, as upstream's `inlineGeneratorCall`
/// places every one: in a loop labelled [`LoopLabel::INLINED_CF_SCOPE`]
/// appended to `caller`, the call's block, whose every return, and every
/// raise when the call has a raise edge, at any depth, breaks that loop as
/// `exit` rewrites it; `resumed` follows the loop. Returns the loop's
/// position.
fn splice_region(
    function: &mut MirFunction,
    caller: &mut MirBlock,
    mut blocks: Vec<MirBlock>,
    resumed: Vec<MirInstr>,
    exit: &ReturnExit,
) -> usize {
    exit.break_returns(function, &mut blocks);
    let at = caller.instrs.len();
    caller.instrs.push(MirInstr::Loop {
        label: Some(LoopLabel::INLINED_CF_SCOPE),
        operands: Vec::new(),
        args: Vec::new(),
        results: exit.results(),
        body: blocks,
    });
    caller.instrs.extend(resumed);
    at
}

/// How an inlined body's returns leave it: through what it returns, and
/// `dest`, the call's register, which a [`Returned::Value`] makes the
/// wrapper loop's result, and how its raises do when the call has a raise
/// edge.
struct ReturnExit {
    result: Returned,
    dest: Reg,
    /// The call's raise edge, whose flag and error the wrapper loop's
    /// results define after the call's register, as upstream's inlined
    /// `lit.error_return` returns the error flag: a return breaks with the
    /// flag false, a `raise` with it true and its error, and the call's
    /// error region after the loop destroys what the caller holds.
    raises: Option<Raises>,
}

impl ReturnExit {
    /// The wrapper loop's results: the call's register when the body
    /// returns a value, then the raise edge's flag and error.
    fn results(&self) -> Vec<Reg> {
        let mut results = if self.result == Returned::Value {
            vec![self.dest]
        } else {
            Vec::new()
        };
        if let Some(raises) = &self.raises {
            results.extend([raises.flag, raises.error]);
        }
        results
    }

    /// Rewrite a return ending `block` into a break of the wrapper loop,
    /// carrying the returned value when the loop has a result. A
    /// value-less return from a function returning a value is the
    /// checker's unreachable fall-off; a break carrying no value cannot
    /// leave a loop with a result, so it is unreachable there. A raise out
    /// of the callee (`lit.error_return`) breaks the loop too when the call
    /// has a raise edge, carrying its error, a string converted to `Error`
    /// as the raise converts it, and leaves the body as it left the callee
    /// when it has none; a raise naming a `try` of the callee stays. A value the
    /// path does not define is upstream's `#interp.uninitmem`.
    fn rewrite(&self, function: &mut MirFunction, block: &mut MirBlock) {
        let span = function
            .spans
            .0
            .get(&self.dest.0)
            .map(|(span, _)| span.clone());
        let constant = |function: &mut MirFunction, block: &mut MirBlock, ty: Ty, k| {
            let dest = fresh_register(function, ty, span.as_ref());
            block.instrs.push(MirInstr::Const { dest, k });
            dest
        };
        let raised = matches!(block.term, MirTerm::Raise { label: None, .. });
        let mut values = match &block.term {
            MirTerm::Return(Some(value)) if self.result == Returned::Value => vec![*value],
            MirTerm::Return(_) if matches!(self.result, Returned::None | Returned::Out) => {
                Vec::new()
            }
            MirTerm::Return(_) => {
                block.term = MirTerm::Unreachable;
                return;
            }
            MirTerm::Raise { label: None, .. } if self.raises.is_some() => {
                if self.result == Returned::Value {
                    let ty = function
                        .reg_types
                        .get(&self.dest.0)
                        .cloned()
                        .unwrap_or_else(Ty::none);
                    vec![constant(function, block, ty, Const::UninitMem)]
                } else {
                    Vec::new()
                }
            }
            _ => return,
        };
        if let Some(raises) = &self.raises {
            values.push(constant(function, block, Ty::bool(), Const::Bool(raised)));
            let error = match block.term {
                MirTerm::Raise {
                    src: error,
                    label: None,
                    ..
                } if function
                    .reg_types
                    .get(&error.0)
                    .is_some_and(|ty| *ty != raises.ty) =>
                {
                    let converted = fresh_register(function, raises.ty.clone(), span.as_ref());
                    block.instrs.push(error_conversion(converted, error));
                    converted
                }
                MirTerm::Raise {
                    src: error,
                    label: None,
                    ..
                } => error,
                _ => constant(function, block, raises.ty.clone(), Const::UninitMem),
            };
            values.push(error);
        }
        block.term = MirTerm::Break {
            label: Some(LoopLabel::INLINED_CF_SCOPE),
            values,
        };
    }

    /// [`Self::rewrite`] every return, and every raise, of `blocks` and
    /// their regions into a break of the [`LoopLabel::INLINED_CF_SCOPE`]
    /// loop, upstream's `hlcf.break` of `inlined_cf_scope`.
    fn break_returns(&self, function: &mut MirFunction, blocks: &mut [MirBlock]) {
        for block in blocks {
            for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
                self.break_returns(function, region);
            }
            self.rewrite(function, block);
        }
    }
}

/// Whether a body inlined at a call whose raise edge is `raises` can route
/// each raise out of the callee in `blocks` into it: one raising a value of the edge's
/// error type, or a string the raise converts to `Error`. `reg_types` are
/// the callee's, whose registers the body numbers from `base`.
fn routes_raises(
    blocks: &[MirBlock],
    raises: &Raises,
    reg_types: &HashMap<u32, Ty>,
    base: u32,
) -> bool {
    blocks.iter().all(|block| {
        let routed = match block.term {
            MirTerm::Raise {
                src: error,
                label: None,
                ..
            } => error
                .0
                .checked_sub(base)
                .and_then(|callee| reg_types.get(&callee))
                .is_some_and(|ty| *ty == raises.ty || converts_to_error(ty, &raises.ty)),
            _ => true,
        };
        routed
            && block
                .instrs
                .iter()
                .flat_map(instruction_regions)
                .all(|region| routes_raises(region, raises, reg_types, base))
    })
}

/// Whether a raised value of type `ty` is a string a raise converts to the
/// error type `error`, `Error`.
fn converts_to_error(ty: &Ty, error: &Ty) -> bool {
    matches!(error.kind(), TyKind::Error)
        && match ty.kind() {
            TyKind::StringLiteral => true,
            TyKind::Struct(name, _) => mojito_symbol::symbol::is_stdlib_string_struct(name),
            _ => false,
        }
}

/// The conversion of the string in `message` to the `Error` in `dest`, as a
/// raise converts it.
fn error_conversion(dest: Reg, message: Reg) -> MirInstr {
    MirInstr::Call {
        dest,
        func: FuncRef::named("Error"),
        raises: None,
        args: vec![message],
        kwargs: Vec::new(),
        arg_places: vec![None],
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: Vec::new(),
        spread: None,
    }
}

/// Retype each read of `var`, a callable slot bound to a caller's closure,
/// in `blocks` and their regions to the closure's type `ty`: a `var.use`'s
/// register in `reg_types`, and a call through the slot's place.
fn retype_closure_slot(
    blocks: &mut [MirBlock],
    var: VarId,
    ty: &Ty,
    reg_types: &mut HashMap<u32, Ty>,
) {
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        for region in instruction_regions_mut(instruction) {
            retype_closure_slot(region, var, ty, reg_types);
        }
        match instruction {
            MirInstr::UseVar {
                dest, var: read, ..
            } if *read == var => {
                reg_types.insert(dest.0, ty.clone());
            }
            MirInstr::CallIndirect {
                callee_place: Some(place),
                ..
            } if place.root == var && place.is_whole_root() => {
                place.root_ty = Some(ty.clone());
                place.ty = Some(ty.clone());
            }
            _ => {}
        }
    }
}

/// Add to `function`'s `callee_removed_params` each callee a call names
/// that it has no entry for, by the removals of the callee's body: the
/// copy or move constructor a copy or move of an inlined body's slot calls
/// once named and lowered, which no inlined callee's snapshot listed, so
/// the verifier reads the call at the constructor's kept parameters as the
/// call-graph pass will.
fn record_lowered_callees(function: &mut MirFunction, callees: &Callees<'_>) {
    let mut named = Vec::new();
    call_targets_in(&function.blocks, &mut named);
    for callee in named {
        if function
            .callee_removed_params
            .binary_search_by(|(name, _)| name.as_str().cmp(&callee))
            .is_ok()
        {
            continue;
        }
        if let Some(body) = callees.unit_body(&callee) {
            merge_callee_removals(
                &mut function.callee_removed_params,
                &[(callee, body.removed_params.clone())],
            );
        }
    }
}

/// The callees the direct calls of `blocks` and their regions name.
fn call_targets_in(blocks: &[MirBlock], out: &mut Vec<String>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::Call { func, .. } = instruction {
            out.push(func.0.clone());
        }
        for region in instruction_regions(instruction) {
            call_targets_in(region, out);
        }
    }
}

/// Add to a caller's `callee_removed_params` an inlined callee's: the
/// inlined body forwards to the callee's callees as the callee did, by
/// their removals then.
fn merge_callee_removals(
    into: &mut Vec<(String, Vec<mojito_types::param_expr::ParamId>)>,
    from: &[(String, Vec<mojito_types::param_expr::ParamId>)],
) {
    for (callee, ids) in from {
        match into.binary_search_by(|(name, _)| name.as_str().cmp(callee)) {
            Ok(at) => {
                let held = &mut into[at].1;
                held.extend(ids.iter().cloned());
                held.sort_by_key(|id| id.slot);
                held.dedup();
            }
            Err(at) => into.insert(at, (callee.clone(), ids.clone())),
        }
    }
}

/// The collector operands a call passes its callee's reduced
/// `declaration`: the spread's position among the kept arguments, which
/// stays the last, and the forwarded `**kwargs^` register, each `None`
/// when the generator removed its collector, which the third value
/// reports as a dropped operand; `None` for a forward into a callee that
/// declares no keyword collector, which the pin rejects.
fn collector_operands(
    spread: Option<usize>,
    forwarded: Option<Reg>,
    declaration: &MirFunctionDeclaration,
    full_declaration: &MirFunctionDeclaration,
    arguments: usize,
) -> Option<(Option<usize>, Option<Reg>, bool)> {
    let mut dropped = false;
    let spread = match spread {
        Some(_) if declaration.variadic.is_some() => Some(arguments.checked_sub(1)?),
        Some(_) => {
            dropped = true;
            None
        }
        None => None,
    };
    let forwarded = match forwarded {
        Some(register) if declaration.kw_variadic.is_some() => Some(register),
        Some(_) if full_declaration.kw_variadic.is_some() => {
            dropped = true;
            None
        }
        Some(_) => return None,
        None => None,
    };
    Some((spread, forwarded, dropped))
}

/// The constant a read of the parameter value `value` at `ty` is: its
/// typed constant when closed, the parameter expression otherwise.
fn parameter_constant(value: &ParamExpr, ty: Option<&Ty>) -> Const {
    value
        .as_constant()
        .and_then(|constant| Const::of_parameter_value(constant, ty))
        .unwrap_or_else(|| Const::Param(value.clone()))
}

/// How the callee's parameter `param` binds the call's `argument`, which
/// the call passes with `place` retained: a `mut` or `ref` parameter binds
/// that place, as the VM binds the parameter's slot to a reference to it,
/// and so does a `read` one of a callee returning a reference, which may
/// be to it; a `deinit` parameter binds the caller's slot the argument
/// forwards or loads whole (an explicit destructor's receiver, whose
/// residue the caller consumes after the call), which the ownership check
/// proved whole there, and is otherwise a fresh slot the argument, an owned
/// value the check proved whole (a copied receiver's move), initializes,
/// for the body to consume;
/// an owned parameter, or a `read` one of a type no destructor tears down,
/// is a fresh slot the argument initializes, and so is a `read` one passed
/// a trivial copy of a slot; any other `read` parameter binds the caller's
/// place the call retained for it or the argument was loaded from, through
/// a reference the block made included, as upstream substitutes the
/// argument's address. `None` when the call retains no place for a `mut`
/// or `ref` one, or a `read` one of a droppable type is read through a
/// reference the block did not make.
fn argument_binding(
    caller: &MirFunction,
    callee: &MirFunction,
    block: &MirBlock,
    index: usize,
    param: usize,
    argument: Reg,
    place: Option<&MirPlace>,
) -> Option<Binding> {
    let owned = callee.owned_params.get(param).copied().unwrap_or(false);
    let deinit = callee.deinit_params.get(param).copied().unwrap_or(true);
    let reference = is_reference_parameter(callee, param);
    if (reference || (callee.returns_reference && !owned && !deinit))
        && let Some(place) = place
    {
        return Some(Binding::Place(place.clone()));
    }
    if deinit && place.is_none() {
        if let Some(slot) = forwarded_slot(block, index, argument) {
            return Some(Binding::Slot(slot));
        }
        let loaded = loaded_place(block, index, argument)
            .or_else(|| loaded_slot_across_regions(block, index, argument))
            .filter(|place| place.through.is_none());
        return Some(match loaded {
            Some(place) if place.is_whole_root() => Binding::Slot(place.root),
            Some(place)
                if is_owned_part(&place)
                    && matches!(
                        block.instrs.get(index + 1),
                        Some(MirInstr::ConsumePlace { place: consumed, marker })
                            if *marker == argument && same_part(consumed, &place)
                    ) =>
            {
                Binding::Part { place, argument }
            }
            _ => Binding::Value(argument),
        });
    }
    if reference {
        return None;
    }
    if owned
        || callee.param_types.get(param).is_some_and(has_no_destructor)
        || trivially_copied(caller, block, index, argument)
    {
        return Some(Binding::Value(argument));
    }
    place
        .cloned()
        .or_else(|| loaded_place(block, index, argument))
        .map(Binding::Place)
}

/// Where the straight-line run holding `block`'s instruction `index`
/// starts: past the last region op before it that lowering splits the
/// block at ([`splits_runs`]). The caller-side lookbacks read only that
/// run, so a structured caller's call binds as its lowered block would.
fn run_start(block: &MirBlock, index: usize) -> usize {
    block.instrs[..index]
        .iter()
        .rposition(splits_runs)
        .map_or(0, |op| op + 1)
}

/// Whether `instruction` ends a straight-line run of its block: every
/// region op but a `try`, whose parts run in the block's order.
const fn splits_runs(instruction: &MirInstr) -> bool {
    region_op(instruction) && !matches!(instruction, MirInstr::Try { .. })
}

/// Whether the register `argument` is a copy of a slot of `caller` in
/// `block` before the call at `index`, in its run ([`run_start`]), that runs no copy constructor, the
/// slot's type closed so that none is named later, or a `struct.create`,
/// which `sroa` builds only of a type whose drop runs no code: an owned
/// value of a type with nothing to destroy, which a `read` parameter
/// borrows whole.
fn trivially_copied(caller: &MirFunction, block: &MirBlock, index: usize, argument: Reg) -> bool {
    block.instrs[run_start(block, index)..index]
        .iter()
        .rev()
        .any(|instruction| match instruction {
            MirInstr::UseVar {
                dest,
                var,
                mode: UseMode::Copy,
                constructor: None,
            } => *dest == argument && caller.var_tys.get(var).is_some_and(Ty::is_closed),
            MirInstr::MakeStruct { dest, .. } => *dest == argument,
            _ => false,
        })
}

/// How the callee's parameter `param`, which the call omits, binds its
/// `default`: a fresh slot the materialized default initializes, dropped
/// after the body unless the parameter owns it; `None` for a reference or
/// `deinit` parameter, or a default the pass does not materialize.
fn default_binding(
    callee: &MirFunction,
    param: usize,
    default: &CheckedConst,
    declarations: &MirDeclarations,
) -> Option<Binding> {
    if is_reference_parameter(callee, param)
        || callee.deinit_params.get(param).copied().unwrap_or(true)
    {
        return None;
    }
    let ty = callee.param_types.get(param)?;
    let value = DefaultValue::of(default, ty, declarations)?;
    let dropped =
        !callee.owned_params.get(param).copied().unwrap_or(false) && !has_no_destructor(ty);
    Some(Binding::Default { value, dropped })
}

/// Turn every part-bound parameter of `bindings` into a fresh slot its
/// argument initializes, for a body that names the parameter other than as
/// a place's root; returns whether one was.
fn demote_parts(bindings: &mut [Binding]) -> bool {
    let mut demoted = false;
    for binding in bindings.iter_mut() {
        if let Binding::Part { argument, .. } = *binding {
            *binding = Binding::Value(argument);
            demoted = true;
        }
    }
    demoted
}

/// Whether the owned parts `left` and `right` are the same part of the
/// same slot.
fn same_part(left: &MirPlace, right: &MirPlace) -> bool {
    left.root == right.root
        && left.through == right.through
        && left.proj.len() == right.proj.len()
        && left.proj.iter().zip(&right.proj).all(|steps| match steps {
            (Proj::Field(left), Proj::Field(right)) => left == right,
            (Proj::ConstIndex(left), Proj::ConstIndex(right)) => left == right,
            _ => false,
        })
}

/// Whether `place` names an owned part of its root slot directly: fields
/// and tuple elements, at least one, and no reference on the way.
fn is_owned_part(place: &MirPlace) -> bool {
    place.through.is_none()
        && !place.proj.is_empty()
        && place
            .proj
            .iter()
            .all(|step| matches!(step, Proj::Field(_) | Proj::ConstIndex(_)))
}

/// The slot the register `argument` forwards in `block` before the call at
/// `index`, in its run ([`run_start`]), when nothing between the forward and the call names it.
fn forwarded_slot(block: &MirBlock, index: usize, argument: Reg) -> Option<VarId> {
    let run = run_start(block, index);
    let (forward, slot) =
        block.instrs[run..index]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(position, instruction)| match instruction {
                MirInstr::UseVar {
                    dest,
                    var,
                    mode: UseMode::Forward,
                    ..
                } if *dest == argument => Some((run + position, *var)),
                _ => None,
            })?;
    block.instrs[forward + 1..index]
        .iter()
        .all(|instruction| !instruction_slots(instruction).contains(&slot))
        .then_some(slot)
}

/// The whole slot the register `argument` is a load of before the call at
/// `index` of `block`, past the region ops between them (an inlined call
/// evaluating a later argument), none of which names the slot at any
/// depth: a `deinit` argument a transfer loads ahead of the call's other
/// arguments, which upstream passes by address.
fn loaded_slot_across_regions(block: &MirBlock, index: usize, argument: Reg) -> Option<MirPlace> {
    let (load, place) =
        block.instrs[..index]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(position, instruction)| match instruction {
                MirInstr::LoadPlace { dest, place } if *dest == argument => Some((position, place)),
                _ => None,
            })?;
    (place.is_whole_root()
        && !block.instrs[load + 1..index]
            .iter()
            .any(|instruction| names_slot(instruction, place.root)))
    .then(|| place.clone())
}

/// Whether `instruction` or, at any depth, its regions name the slot `slot`.
fn names_slot(instruction: &MirInstr, slot: VarId) -> bool {
    instruction_slots(instruction).contains(&slot)
        || mojito_mir::mir::instruction_regions(instruction)
            .into_iter()
            .flatten()
            .flat_map(|block| &block.instrs)
            .any(|instruction| names_slot(instruction, slot))
}

/// Whether `collector` is the positional collector the call at `index` of
/// `block` builds: a `tuple.make` in the straight-line run before it, or the
/// `struct.create` of a homogeneous collector's `VariadicList` over one.
fn builds_collector(block: &MirBlock, index: usize, collector: Reg) -> bool {
    block.instrs[run_start(block, index)..index]
        .iter()
        .any(|instruction| match instruction {
            MirInstr::MakeTuple { dest, .. } | MirInstr::MakeStruct { dest, .. } => {
                *dest == collector
            }
            _ => false,
        })
}

/// Whether the callee's parameter `param` is a `mut` or `ref` one, whose
/// slot the VM binds to a reference to the caller's place.
fn is_reference_parameter(callee: &MirFunction, param: usize) -> bool {
    callee.ref_params.get(param).copied().unwrap_or(false)
}

/// Whether every reference `callee` returns is to a place its parameters'
/// `bindings` give it from the caller: each returned register a reference
/// made to a place-bound parameter's place, or any reference when every
/// parameter is place-bound, since one to a local of its own the checker
/// rejects. A reference to a fresh slot an argument initializes keeps the
/// call, as the caller's loan names the argument's place, not that slot. A
/// reference to a pointee, an element a pointer the body holds addresses
/// (`elements[unsafe_offset=i][]`), names no slot of the body at all.
fn returns_bound_place(callee: &MirFunction, bindings: &[Binding]) -> bool {
    let place_bound = |var: VarId| matches!(bindings.get(var as usize), Some(Binding::Place(_)));
    if (0..callee.n_params as VarId).all(place_bound) {
        return true;
    }
    let pointee = |place: &MirPlace| {
        matches!(
            place.proj.first(),
            Some(Proj::Index(_) | Proj::ConstIndex(_))
        ) && matches!(
            place.root_ty.as_ref().map(Ty::kind),
            Some(TyKind::Pointer { .. })
        )
    };
    let mut made = HashMap::new();
    let mut returned = Vec::new();
    references_and_returns(&callee.blocks, &mut made, &mut returned);
    returned.iter().all(|value| {
        made.get(&value.0).is_some_and(|place: &&MirPlace| {
            pointee(place)
                || place.through.is_none_or(|var| var == place.root) && place_bound(place.root)
        })
    })
}

/// Add to `made` the place each reference `blocks` and their regions make
/// refers to, by its register, and to `returned` each register they return.
fn references_and_returns<'b>(
    blocks: &'b [MirBlock],
    made: &mut HashMap<u32, &'b MirPlace>,
    returned: &mut Vec<Reg>,
) {
    for block in blocks {
        for instruction in &block.instrs {
            if let MirInstr::MakeRef { dest, place } = instruction {
                made.insert(dest.0, place);
            }
            for region in instruction_regions(instruction) {
                references_and_returns(region, made, returned);
            }
        }
        if let MirTerm::Return(Some(value)) = block.term {
            returned.push(value);
        }
    }
}

/// Add to `returned` each value a `break` of `blocks`, at any depth,
/// carries for a loop result `returned` holds, then each operand and
/// `continue` value defining a loop argument it holds, outer loops first,
/// so a nested loop's result an outer break carries brings its own breaks'.
fn carried_by_breaks(blocks: &mut [MirBlock], returned: &mut Vec<Reg>) {
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        if let MirInstr::Loop {
            label,
            operands,
            args,
            results,
            body,
        } = instruction
        {
            for (at, result) in results.iter().enumerate() {
                if returned.contains(result) {
                    loop_breaks_mut(body, *label, &mut |values, _| {
                        returned.extend(values.get(at));
                    });
                }
            }
            for (at, arg) in args.iter().enumerate() {
                if returned.contains(arg) {
                    returned.extend(operands.get(at));
                    loop_continues_mut(body, *label, &mut |values, _| {
                        returned.extend(values.get(at));
                    });
                }
            }
        }
        for region in instruction_regions_mut(instruction) {
            carried_by_breaks(region, returned);
        }
    }
}

/// Whether the callee has a shape the splice handles: a struct receiver
/// if any, and blocks of plain or structured control flow, at any depth,
/// with no `try` (which the pin inlines, R1288), closure, or compile-time
/// assertion (which fails the callee's own instance).
fn is_closed(callee: &MirFunction, declaration: &MirFunctionDeclaration) -> bool {
    let receiver_closed = !declaration.has_receiver
        || callee
            .param_types
            .first()
            .is_some_and(|receiver| matches!(receiver.kind(), TyKind::Struct(..)));
    receiver_closed && closed_blocks(&callee.blocks)
}

/// Whether `blocks` and their regions hold no `try`, closure, compile-time
/// assertion, or `try`-crossing terminator.
fn closed_blocks(blocks: &[MirBlock]) -> bool {
    blocks.iter().all(|block| {
        block.instrs.iter().all(|instruction| {
            !matches!(
                instruction,
                MirInstr::Try { .. }
                    | MirInstr::ComptimeAssert { .. }
                    | MirInstr::MakeClosure { .. }
            ) && instruction_regions(instruction)
                .into_iter()
                .all(|region| closed_blocks(region))
        }) && !matches!(block.term, MirTerm::TryYield(_))
    })
}

/// The place the register `argument` was loaded from in `block` before the
/// call at `index`, in its run ([`run_start`]), directly or through a reference the block made to it
/// (`self.copy()` in `__iter__`, a `ref.make` of `self` read by the
/// `copy=` argument), when nothing between the load, or the reference's
/// making, and the call names the place's root but another load.
fn loaded_place(block: &MirBlock, index: usize, argument: Reg) -> Option<MirPlace> {
    let run = run_start(block, index);
    let defining = |reg: Reg| {
        block.instrs[run..index]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(position, instruction)| match instruction {
                MirInstr::LoadPlace { dest, place } if *dest == reg => {
                    Some((run + position, place))
                }
                MirInstr::ReadRef { dest, reference } if *dest == reg => block.instrs
                    [run..run + position]
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(made, instruction)| match instruction {
                        MirInstr::MakeRef { dest, place } if dest == reference => {
                            Some((run + made, place))
                        }
                        _ => None,
                    }),
                _ => None,
            })
    };
    let (load, place) = defining(argument)?;
    let roots = [Some(place.root), place.through];
    block.instrs[load + 1..index]
        .iter()
        .all(|instruction| {
            matches!(instruction, MirInstr::LoadPlace { .. })
                || !instruction_slots(instruction)
                    .iter()
                    .any(|var| roots.contains(&Some(*var)))
        })
        .then(|| place.clone())
}

/// The callee's place `inner`, rooted at a parameter bound to the caller's
/// place `outer`, as a place of the caller: a reference parameter's own
/// reference becomes whatever reaches `outer`.
fn composed_place(outer: &MirPlace, inner: &MirPlace) -> MirPlace {
    if inner.proj.is_empty() {
        return outer.clone();
    }
    let through = inner.through.filter(|&var| var != inner.root);
    let mut place = outer.clone();
    place.proj.extend(inner.proj.iter().cloned());
    place
        .projection_tys
        .extend(inner.projection_tys.iter().cloned());
    place.ty.clone_from(&inner.ty);
    place.through = outer.through.or(through);
    place
}

/// Rewrite each copy of a parameter `bound` to a caller's place in
/// `blocks`, `callee`'s body with its slots renumbered, and their regions,
/// into a load of that place and a copy of the loaded value, which names
/// the copy constructor the parameter's instantiated type has once the
/// splice names constructors, as a copy of a slot does: upstream's copy of
/// a `read` argument reads it through its address.
fn copy_bound_parameters<'b>(
    blocks: &mut [MirBlock],
    callee: &mut MirFunction,
    bound: &dyn Fn(VarId) -> Option<&'b MirPlace>,
) {
    for block in blocks {
        let mut rewritten = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            for region in instruction_regions_mut(&mut instruction) {
                copy_bound_parameters(region, callee, bound);
            }
            match instruction {
                MirInstr::UseVar {
                    dest,
                    var,
                    mode: UseMode::Copy,
                    constructor,
                } if let Some(outer) = bound(var) => {
                    let ty = callee.reg_types.get(&dest.0).cloned();
                    let span = callee.spans.0.get(&dest.0).map(|(span, _)| span.clone());
                    let loaded = fresh_register(
                        callee,
                        ty.unwrap_or_else(|| outer.ty.clone().unwrap_or_else(Ty::none)),
                        span.as_ref(),
                    );
                    rewritten.push(MirInstr::LoadPlace {
                        dest: loaded,
                        place: outer.clone(),
                    });
                    rewritten.push(MirInstr::CopyValue {
                        dest,
                        value: loaded,
                        constructor,
                    });
                }
                other => rewritten.push(other),
            }
        }
        block.instrs = rewritten;
    }
}

/// Remove from `blocks` and their regions each consume of a slot
/// `part_bound` answers for: the caller's consumption of a part-bound
/// parameter's place after the call is the body's end of it.
fn retain_unconsumed(blocks: &mut [MirBlock], part_bound: &dyn Fn(VarId) -> bool) {
    for block in blocks {
        block.instrs.retain(
            |instruction| !matches!(instruction, MirInstr::ConsumeVar { var } if part_bound(*var)),
        );
        for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
            retain_unconsumed(region, part_bound);
        }
    }
}

/// Move the registers of `blocks` and their regions to the caller's by
/// `registers`, and compose each place rooted at a parameter `bound` to a
/// caller's place with it; `None` when such a parameter is named other
/// than as a place's root or the whole value an iteration or a loan's
/// interior reads, or a place reached through a reference other than the
/// parameter's composes with a caller's place reached through one.
fn rebind_blocks<'b>(
    blocks: &mut [MirBlock],
    registers: &Registers,
    bound: &dyn Fn(VarId) -> Option<&'b MirPlace>,
) -> Option<()> {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in instruction_regions_mut(instruction) {
                rebind_blocks(region, registers, bound)?;
            }
            for reg in instruction_regs_mut(instruction) {
                *reg = registers.of(*reg);
            }
            for place in instruction_places_mut(instruction) {
                let through_other = place.through.filter(|&var| var != place.root);
                match bound(place.root) {
                    Some(outer) if through_other.is_some() && outer.through.is_some() => {
                        return None;
                    }
                    Some(outer) => *place = composed_place(outer, place),
                    None if through_other.is_some_and(|var| bound(var).is_some()) => {
                        return None;
                    }
                    None => {}
                }
            }
            // An iteration over a place-bound parameter iterates the
            // caller's slot, and a loan's interior of the parameter is an
            // interior of that slot.
            match instruction {
                MirInstr::GetIter { source, .. }
                    if let Some(outer) = bound(*source)
                        && outer.is_whole_root()
                        && outer.through.is_none() =>
                {
                    *source = outer.root;
                }
                MirInstr::EstablishLoans {
                    loans,
                    dest_interior,
                    ..
                } => {
                    for interior in loans
                        .iter_mut()
                        .filter_map(|loan| loan.interior.as_mut())
                        .chain(dest_interior.as_mut())
                    {
                        if let Some(outer) = bound(interior.root)
                            && outer.is_whole_root()
                            && outer.through.is_none()
                        {
                            interior.root = outer.root;
                        }
                    }
                }
                _ => {}
            }
            if instruction_slots(instruction)
                .into_iter()
                .any(|var| bound(var).is_some())
            {
                return None;
            }
        }
        for reg in terminator_regs_mut(&mut block.term) {
            *reg = registers.of(*reg);
        }
    }
    Some(())
}

/// The [`inline_cost`] of `blocks` and their regions but for their back
/// edges, each constant of `constants` counted once.
fn blocks_cost(
    blocks: &[MirBlock],
    callees: &Callees<'_>,
    constants: &mut BTreeSet<String>,
) -> usize {
    blocks
        .iter()
        .map(|block| {
            let mut loaded = Vec::new();
            block
                .instrs
                .iter()
                .map(|instruction| instruction_cost(instruction, callees, constants, &mut loaded))
                .sum::<usize>()
        })
        .sum()
}

/// What `instruction` and its regions cost toward [`inline_cost`], beside
/// `loaded`, the places its block has read so far.
fn instruction_cost<'i>(
    instruction: &'i MirInstr,
    callees: &Callees<'_>,
    constants: &mut BTreeSet<String>,
    loaded: &mut Vec<&'i MirPlace>,
) -> usize {
    let regions = |constants: &mut BTreeSet<String>| {
        instruction_regions(instruction)
            .into_iter()
            .map(|region| blocks_cost(region, callees, constants))
            .sum::<usize>()
    };
    match instruction {
        MirInstr::Const { k, .. } => usize::from(constants.insert(format!("{k:?}"))),
        MirInstr::UseVar { constructor, .. } => usize::from(constructor.is_some()),
        MirInstr::DropVar { destructor, .. } => usize::from(destructor.is_some()),
        MirInstr::LoadPlace { place, .. } => {
            loaded.push(place);
            1 + address_cost(place)
        }
        MirInstr::Store { place, .. } => match address_cost(place) {
            0 => 0,
            _ if loaded.iter().any(|read| same_part(read, place)) => 1,
            address => 1 + address,
        },
        MirInstr::DefVar { .. }
        | MirInstr::DeclareVar { .. }
        | MirInstr::Drop { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::ComptimeAssert { .. } => 0,
        MirInstr::GetIter { prepare, .. } => prepare
            .iter()
            .map(|method| protocol_call_cost(method, callees))
            .sum(),
        MirInstr::Call { func, .. } if func.0 == "print" => PRINT_COST,
        MirInstr::Try { .. } => TRY_COST + regions(constants),
        MirInstr::If { elifs, .. } => 1 + elifs.len() + regions(constants),
        MirInstr::Loop { .. }
        | MirInstr::ComptimeIf { .. }
        | MirInstr::ComptimeFor { .. }
        | MirInstr::Match { .. } => regions(constants),
        _ => 1,
    }
}

/// The `struct.gep`s the pin computes `place`'s address with: one per
/// projection, `rebind` reusing its operand's address.
fn address_cost(place: &MirPlace) -> usize {
    place
        .proj
        .iter()
        .filter(|projection| !matches!(projection, Proj::Rebind))
        .count()
}

/// What a `for` loop's `__iter__` call costs toward [`inline_cost`]: the
/// operations of the body this pass follows, which the pin's lowering calls
/// and inlines before it measures, or one for a call that stays.
fn protocol_call_cost(method: &str, callees: &Callees<'_>) -> usize {
    callees
        .body(method)
        .map_or(1, |body| inline_cost(body, callees))
}

/// How many `continue`s of `blocks`, a structured body, are reached, each
/// the back edge lowering makes of it.
fn structured_back_edges(blocks: &[MirBlock]) -> usize {
    let mut walk = ContinueWalk::default();
    walk.region(blocks);
    walk.continues
}

/// The walk [`structured_back_edges`] counts with: the frames enclosing
/// the region being walked, innermost last, each beside whether a reached
/// terminator leaves it and whether one resumes it.
#[derive(Default)]
struct ContinueWalk {
    frames: Vec<(RegionFrame, bool, bool)>,
    continues: usize,
}

impl ContinueWalk {
    /// Walk `blocks`, a reached region, whose one block holds it; returns
    /// whether its `yield` or elif condition's end is reached. A block's
    /// end is reached when each region op before it completes.
    fn region(&mut self, blocks: &[MirBlock]) -> bool {
        let Some(block) = blocks.first() else {
            return true;
        };
        if !block
            .instrs
            .iter()
            .all(|instruction| self.completes(instruction))
        {
            return false;
        }
        if let Some((target, resume)) = region_exit(&block.term) {
            if resume && matches!(target, RegionFrame::Loop(_) | RegionFrame::ComptimeFor) {
                self.continues += 1;
            }
            if let Some((_, left, resumed)) = self
                .frames
                .iter_mut()
                .rev()
                .find(|(kind, ..)| target.names(*kind))
            {
                *if resume { resumed } else { left } = true;
            }
        }
        matches!(
            block.term,
            MirTerm::Yield(_) | MirTerm::ComptimeYield(_) | MirTerm::ElifCond { .. }
        )
    }

    /// Walk the reached `instruction`'s regions; returns whether control
    /// continues after it. An `if` or `comptime if` completes when a
    /// reached arm yields, a loop when a reached `break` leaves it, and a
    /// `match` when a reached case completes or its else region yields, a
    /// case reached when the one before it reaches `match.next`.
    fn completes(&mut self, instruction: &MirInstr) -> bool {
        match instruction {
            MirInstr::If {
                then,
                elifs,
                orelse,
                ..
            } => {
                let mut completes = self.region(then);
                let mut rest = true;
                for elif in elifs {
                    rest = rest && self.region(&elif.cond);
                    completes |= rest && self.region(&elif.then);
                }
                (rest && self.region(orelse)) | completes
            }
            MirInstr::ComptimeIf { arms, orelse, .. } => arms
                .iter()
                .chain(std::iter::once(orelse))
                .fold(false, |completes, arm| self.region(arm) | completes),
            MirInstr::Loop { label, body, .. } => self.framed(RegionFrame::Loop(*label), body).0,
            MirInstr::ComptimeFor { body, .. } => self.framed(RegionFrame::ComptimeFor, body).0,
            MirInstr::Match { cases, orelse, .. } => {
                let mut completes = false;
                for case in cases {
                    let (completed, next) = self.framed(RegionFrame::MatchCase, &case.body);
                    completes |= completed;
                    if !next {
                        return completes;
                    }
                }
                self.region(orelse) | completes
            }
            _ => true,
        }
    }

    /// Walk `blocks` under a fresh frame of `kind`; returns whether a
    /// reached terminator leaves it and whether one resumes it.
    fn framed(&mut self, kind: RegionFrame, blocks: &[MirBlock]) -> (bool, bool) {
        self.frames.push((kind, false, false));
        self.region(blocks);
        self.frames
            .pop()
            .map_or((false, false), |(_, left, resumed)| (left, resumed))
    }
}

/// What the callee returns through: nothing when it returns `None`, else
/// the wrapper loop's result.
fn result_of(callee: &MirFunction) -> Returned {
    if callee
        .ret_ty
        .as_ref()
        .is_none_or(|ty| matches!(ty.kind(), TyKind::None))
    {
        Returned::None
    } else {
        Returned::Value
    }
}

/// `callee` with its own compile-time parameters bound to the call's
/// `arguments`, and its `owner`'s to the owner's type: the callee itself
/// when it declares none and has no parametric owner, `None` when the
/// binding keeps the call.
fn instantiated<'c>(
    callee: &'c MirFunction,
    declaration: &MirFunctionDeclaration,
    owner: Option<&OwnerInstance<'_>>,
    arguments: &Arguments<'_>,
    declarations: &MirDeclarations,
) -> Option<Instantiated<'c>> {
    if owner.is_none() && declaration.param_decls.is_empty() {
        return arguments.binding().is_empty().then_some(Instantiated {
            body: Cow::Borrowed(callee),
            closures: Vec::new(),
        });
    }
    let mut instantiation = Instantiation::new(declaration, owner, arguments)?;
    let mut function = callee.clone();
    walk_function_mut(&mut instantiation, &mut function).ok()?;
    construct_type_parameters(
        &mut function.blocks,
        &mut function.n_regs,
        &mut function.reg_types,
        &instantiation,
        declarations,
    )
    .ok()?;
    if constructs_binder(&function.blocks, &instantiation.types) {
        return None;
    }
    let closures = instantiation.read_binders(&mut function)?;
    let mut blocks = std::mem::take(&mut function.blocks);
    fold_witnesses(&mut blocks, &function, declarations);
    function.blocks = blocks;
    Some(Instantiated {
        body: Cow::Owned(function),
        closures,
    })
}

/// Fold each witness lookup the substitution closed in `blocks`.
///
/// `blocks` is the body of `function`, whose own blocks are taken. A call
/// through a bound whose receiver the substitution made a struct is
/// respelled to the member that struct's witness table records for the
/// call's requirement, as upstream's substitution folds the witness lookup:
/// a method call takes the member's own compile-time parameter
/// declarations, which its arguments bind by position as they bound the
/// requirement's. A call whose result the checker adapts, or whose result
/// ABI or parameter count differs from the member's, keeps the dispatch.
fn fold_witnesses(blocks: &mut [MirBlock], function: &MirFunction, declarations: &MirDeclarations) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in instruction_regions_mut(instruction) {
                fold_witnesses(region, function, declarations);
            }
            let Some(member) = witness_of(instruction, function, declarations) else {
                continue;
            };
            let Some(declaration) = declarations.function(&member) else {
                continue;
            };
            match instruction {
                MirInstr::MethodCall {
                    resolved,
                    reference_result,
                    result_adapter: None,
                    param_decls,
                    ..
                } if declaration.returns_reference == reference_result.is_some()
                    && declaration.param_decls.len() == param_decls.len() =>
                {
                    *resolved = Some(member);
                    param_decls.clone_from(&declaration.param_decls);
                }
                MirInstr::BinOp { resolved, .. } if declaration.param_decls.is_empty() => {
                    *resolved = Some(member);
                }
                instruction => {
                    if let Some(call) = instruction_subscript_call_mut(instruction)
                        && declaration.param_decls.len() == call.param_decls.len()
                    {
                        call.target = member;
                        call.param_decls.clone_from(&declaration.param_decls);
                    }
                }
            }
        }
    }
}

/// The member `instruction`'s receiver's witness table records for the
/// requirement it calls through a bound, when it calls one.
fn witness_of(
    instruction: &MirInstr,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Option<String> {
    let requirement = match instruction {
        MirInstr::MethodCall {
            resolved: Some(target),
            ..
        }
        | MirInstr::BinOp {
            resolved: Some(target),
            ..
        } => target,
        instruction => &instruction_subscript_call(instruction)?.target,
    };
    if !mojito_symbol::symbol::is_trait_dispatch_symbol(requirement) {
        return None;
    }
    let receiver = member_receiver_ty(function, instruction)?;
    declarations
        .witness(receiver, requirement)
        .map(str::to_string)
}

/// The compile-time arguments the subscript `call` at `index` of `block`
/// passes its accessor's own value parameters: each the constant its
/// register holds there, or the expression over the caller's binders it
/// records, a literal index taking the value it materializes to at the
/// parameter's `Int`; `None` when one is neither.
fn subscript_parameter_arguments(
    block: &MirBlock,
    index: usize,
    call: &MirSubscriptCall,
) -> Option<Vec<TyArg>> {
    let preceding = &block.instrs[run_start(block, index)..index];
    call.param_arg_regs
        .iter()
        .map(|argument| {
            match (
                argument
                    .value
                    .and_then(|reg| defined_constant(preceding, reg)),
                &argument.expr,
            ) {
                (Some(Const::Int(value)), _) => Some(CtValue::Int(*value)),
                (Some(Const::IntLiteral(literal)), _) => literal.to_i64().map(CtValue::Int),
                (Some(Const::Bool(value)), _) => Some(CtValue::Bool(*value)),
                (_, Some(expr)) => Some(CtValue::Expr(expr.clone())),
                _ => None,
            }
            .map(TyArg::Val)
        })
        .collect()
}

/// The constant the last of `instructions` defining `reg` gives it, through
/// a literal's materialization.
fn defined_constant(instructions: &[MirInstr], reg: Reg) -> Option<&Const> {
    instructions
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            MirInstr::Const { dest, k } if *dest == reg => Some(Some(k)),
            MirInstr::MaterializeLiteral { dest, value, .. } if *dest == reg => {
                Some(defined_constant(instructions, *value))
            }
            _ => None,
        })
        .flatten()
}

/// `arguments`, an application's of a struct declaring `decls`, one per
/// binder: a struct keyed on its lone pack and applied element by element
/// (`Tuple[Int, String]`) binds the pack to the element list, as the
/// elaborator binds it, its literal-typed elements materialized; `None`
/// when the arguments do not cover the binders.
fn owner_binder_arguments<'a>(
    decls: &[ParamDecl],
    arguments: &'a [TyArg],
) -> Option<Cow<'a, [TyArg]>> {
    match decls {
        [ParamDecl::Type { variadic: true, .. }]
            if !matches!(arguments, [TyArg::Val(_)])
                && pack_spread_argument(arguments).is_none()
                && list_spread_argument(arguments).is_none() =>
        {
            let elements = arguments
                .iter()
                .filter_map(|argument| match argument {
                    TyArg::Ty(ty) => Some(Some(CtValue::Type(default_literal(ty)))),
                    TyArg::Origin(_) => None,
                    TyArg::Val(_) => Some(None),
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Cow::Owned(vec![TyArg::Val(CtValue::Tuple(elements))]))
        }
        _ => {
            let binding: Vec<TyArg> = without_origins(arguments).cloned().collect();
            (binding.len() == decls.len()).then_some(Cow::Owned(binding))
        }
    }
}

/// `arguments` without its origin arguments, which bind no binder: a
/// declaration's binders are its type and value parameters, and its
/// origin parameters stay symbolic in the body as in an instance of it.
fn without_origins(arguments: &[TyArg]) -> impl Iterator<Item = &TyArg> {
    arguments
        .iter()
        .filter(|argument| !matches!(argument, TyArg::Origin(_)))
}

/// What a callable value binder's argument register holds.
enum CallableArgument {
    /// A function capturing nothing.
    Function(String),
    /// A closure of reference captures the caller makes, at its register.
    Closure(Reg),
}

/// A call's compile-time arguments, each beside the capture-free function
/// its runtime register names when it has one: what a callable value
/// binder (`f: def(Int) -> Int`) is bound to.
struct Arguments<'a> {
    arguments: &'a [TyArg],
    /// Aligned with `arguments`; shorter when the call passes no registers.
    callables: &'a [Option<CallableArgument>],
}

impl Arguments<'_> {
    /// The arguments that bind a binder, origins left out, each with its
    /// function.
    fn binding(&self) -> Vec<(&TyArg, Option<&CallableArgument>)> {
        self.arguments
            .iter()
            .enumerate()
            .filter(|(_, argument)| !matches!(argument, TyArg::Origin(_)))
            .map(|(index, argument)| (argument, self.callables.get(index).and_then(Option::as_ref)))
            .collect()
    }
}

/// Rewrite each read of an enclosing declaration's value binder in
/// `callee`, a nested `def`'s body, which spells it as a slot of its own,
/// into a reference to that binder, which the caller, a body in the
/// binder's scope, reads as its own; `None` when such a slot is used
/// otherwise.
fn read_enclosing_binders(
    callee: &mut MirFunction,
    declaration: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
) -> Option<()> {
    let mut shadowed: Vec<&str> = declaration
        .param_decls
        .iter()
        .map(ParamDecl::name)
        .collect();
    let mut enclosing = declaration.enclosing.as_deref();
    while let Some(name) = enclosing {
        let scope = declarations.function(name)?;
        for decl in &scope.param_decls {
            let ParamDecl::Value {
                id,
                name,
                ty,
                variadic: false,
                ..
            } = decl
            else {
                continue;
            };
            if shadowed.contains(&name.as_str())
                || matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
            {
                continue;
            }
            shadowed.push(name);
            let Some(slot) = callee
                .var_names
                .iter()
                .skip(callee.n_params)
                .position(|var| var == name)
            else {
                continue;
            };
            let value =
                ParamContext::detached().decl_ref(id.clone(), name, MetaTy::value(ty.clone()));
            read_binder(
                &mut callee.blocks,
                (slot + callee.n_params) as VarId,
                &SlotValue::Parameter(&value),
                &callee.reg_types,
            )?;
        }
        enclosing = scope.enclosing.as_deref();
    }
    Some(())
}

/// What a binder was bound to.
enum Bound {
    Type,
    /// A value binder's argument.
    Value(ParamExpr),
    /// A callable value binder's argument: the function it names.
    Callable(String),
    /// A callable value binder's argument: the register of the closure of
    /// reference captures the caller makes.
    Closure(Reg),
}

/// A callee's compile-time parameters bound to a call's arguments, as an
/// environment of [`replace_parameters`].
struct Instantiation {
    context: ParamContext,
    bindings: ParamBindings,
    /// The owner's name and the type the body's bare spelling of it
    /// denotes.
    owner: Option<(String, Ty)>,
    /// Each value binder's spelling, which names the slot a read of it
    /// loads, and its argument.
    values: Vec<(String, ParamExpr)>,
    /// Each owner value binder's spelling, which names the receiver's
    /// member a read of it loads, and its argument.
    members: Vec<(String, ParamExpr)>,
    /// Each callable value binder's spelling, which names the slot the
    /// body reads it from, and the function it is bound to.
    callables: Vec<(String, String)>,
    /// Each callable value binder's spelling, which names the slot the
    /// body reads it from, and the caller's register of the closure it is
    /// bound to.
    closures: Vec<(String, Reg)>,
    /// The callee's type binders, which a site may name only to rename.
    types: Vec<ParamRef>,
}

impl Instantiation {
    /// The binding of `owner`'s binders to its type's arguments, and of
    /// `declaration`'s own to `arguments`, one per binder in declaration
    /// order, which may restate the owner's first; `None` for a binder or
    /// argument this pass does not bind.
    ///
    /// A binder's constraints, the declaration's `where` clauses, and its
    /// availability as a conditional member bind nothing: the checker
    /// judged them at the call, as the pin judges a clause there (a call
    /// from a generic caller without the evidence is rejected as "lacking
    /// evidence to prove correctness"), so the inlined body carries none.
    fn new(
        declaration: &MirFunctionDeclaration,
        owner: Option<&OwnerInstance<'_>>,
        arguments: &Arguments<'_>,
    ) -> Option<Self> {
        let decls = &declaration.param_decls;
        let covered = owner.map_or(0, |owner| restated_owner_binders(owner.decls, decls));
        let own = &decls[covered..];
        let arguments = arguments.binding();
        let own_arguments = if arguments.len() == decls.len() {
            &arguments[covered..]
        } else if arguments.len() == own.len() {
            arguments.as_slice()
        } else {
            return None;
        };
        let mut instantiation = Self {
            bindings: ParamBindings::new(),
            owner: owner.map(|owner| (owner.template.clone(), owner.ty.clone())),
            values: Vec::new(),
            members: Vec::new(),
            callables: Vec::new(),
            closures: Vec::new(),
            types: Vec::new(),
            context: ParamContext::detached(),
        };
        if let Some(owner) = owner {
            let TyKind::Struct(_, owner_arguments) = owner.ty.kind() else {
                return None;
            };
            let owner_arguments = owner_binder_arguments(owner.decls, owner_arguments)?;
            for (decl, argument) in owner.decls.iter().zip(owner_arguments.iter()) {
                match instantiation.bind(decl, argument, None)? {
                    Bound::Value(value)
                        if !owner.fields.iter().any(|(field, _)| field == decl.name()) =>
                    {
                        instantiation.members.push((decl.name().to_string(), value));
                    }
                    Bound::Callable(_) | Bound::Closure(_) => return None,
                    Bound::Value(_) | Bound::Type => {}
                }
            }
        }
        for (decl, (argument, callable)) in own.iter().zip(own_arguments) {
            let name = decl.name().trim_start_matches('*').to_string();
            match instantiation.bind(decl, argument, *callable)? {
                Bound::Value(value) => instantiation.values.push((name, value)),
                Bound::Callable(function) => instantiation.callables.push((name, function)),
                Bound::Closure(closure) => instantiation.closures.push((name, closure)),
                Bound::Type => {}
            }
        }
        Some(instantiation)
    }

    /// Bind the binder `decl` to `argument`, a value binder of function
    /// type to the function `callable` its register names; `None` for a
    /// binder or argument this pass does not bind: a value only the
    /// elaborator folds (a deferred or marker value), a callable the
    /// caller's dataflow does not name, or an owner pack over a computed
    /// list.
    fn bind(
        &mut self,
        decl: &ParamDecl,
        argument: &TyArg,
        callable: Option<&CallableArgument>,
    ) -> Option<Bound> {
        match (decl, argument) {
            (
                ParamDecl::Type {
                    id,
                    name,
                    variadic: false,
                    ..
                },
                TyArg::Ty(ty),
            ) => {
                // A parameter expression naming the binder (a thunk applied
                // to it) reads its type as a shape, closed or not.
                self.bindings
                    .bind(id.clone(), self.context.type_shape(ty.clone()));
                self.bindings.bind_type(id.clone(), ty.clone());
                self.types.push(ParamRef {
                    id: id.clone(),
                    name: name.as_str().into(),
                });
                Some(Bound::Type)
            }
            // A pack binds as the elaborator binds it: its element list as
            // a value, one over the caller's binders included, and as the
            // runtime pack a spread of it expands to; a
            // caller's own pack it forwards (`f[*Us]()`) binds as that pack.
            (
                ParamDecl::Type {
                    id,
                    name,
                    variadic: true,
                    ..
                },
                argument,
            ) => {
                let (value, ty) = match argument {
                    TyArg::Val(CtValue::Tuple(elements)) => {
                        let elements = elements
                            .iter()
                            .map(|element| match element {
                                CtValue::Type(ty) => Some(ty.clone()),
                                _ => None,
                            })
                            .collect::<Option<Vec<_>>>()?;
                        (
                            self.context.type_list(elements.clone()).ok()?,
                            TyKind::RuntimePack(elements).into(),
                        )
                    }
                    TyArg::Ty(pack) => {
                        let TyKind::Param { binder, .. } =
                            pack_spread(std::slice::from_ref(pack))?.kind()
                        else {
                            return None;
                        };
                        let value = self.context.decl_ref(
                            binder.id.clone(),
                            binder.name.trim_start_matches('*'),
                            MetaTy::ParamList(Box::new(MetaTy::Type)),
                        );
                        (value, pack.clone())
                    }
                    _ => return None,
                };
                self.bindings.bind(id.clone(), value);
                self.bindings.bind_type(id.clone(), ty);
                self.types.push(ParamRef {
                    id: id.clone(),
                    name: name.as_str().into(),
                });
                Some(Bound::Type)
            }
            // A callable value binder is a runtime slot of the body the VM
            // reifies from the call's register; bound to the function that
            // register names, as upstream substitutes the parameter value.
            (
                ParamDecl::Value {
                    ty,
                    callable_default: None,
                    ..
                },
                TyArg::Val(CtValue::Deferred(_)),
            ) if matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)) => {
                callable.map(|callable| match callable {
                    CallableArgument::Function(function) => Bound::Callable(function.clone()),
                    CallableArgument::Closure(closure) => Bound::Closure(*closure),
                })
            }
            (
                ParamDecl::Value {
                    id,
                    ty,
                    callable_default: None,
                    ..
                },
                TyArg::Val(value),
            ) if !matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
                && !matches!(value, CtValue::Deferred(_) | CtValue::Marker(_)) =>
            {
                let value = self.context.constant(value.clone()).ok()?;
                self.bindings.bind(id.clone(), value.clone());
                Some(Bound::Value(value))
            }
            _ => None,
        }
    }

    /// `ty` with the owner's bare spelling replaced by the owner's type,
    /// then every bound binder by its argument; an error when a bound
    /// binder survives the substitution (a pack queried under a loop
    /// binder, which only the elaborator's unrolling closes).
    fn instantiate(&self, ty: &Ty) -> Result<Ty, ParamError> {
        let ty = match &self.owner {
            Some((template, owner)) => rewrite_ty(ty, &mut BareOwner { template, owner })?,
            None => ty.clone(),
        };
        let ty = rewrite_ty(
            &replace_parameters(&self.context, &ty, &self.bindings, 0)?,
            &mut SelectedShapes,
        )?;
        if let Some(binder) = self.types.iter().find(|binder| names_binder(&ty, binder)) {
            return Err(ParamError::Unsupported(format!(
                "`{ty}` still names the bound binder `{}`",
                binder.name
            )));
        }
        Ok(ty)
    }

    /// Rewrite each read of a value binder's slot in `function` into its
    /// argument as a constant, a callable's into the function it names,
    /// and delete the slot's drops: the slot holds nothing. A callable
    /// bound to a caller's closure keeps its slot, which the inlined entry
    /// stores the closure into, as upstream substitutes the closure value
    /// for the parameter: returns each such slot with the closure's
    /// register. `None` when a binder's slot is used otherwise.
    fn read_binders(&self, function: &mut MirFunction) -> Option<Vec<(VarId, Reg)>> {
        for (name, value) in &self.values {
            let Some(slot) = function.var_names.iter().position(|var| var == name) else {
                continue;
            };
            read_binder(
                &mut function.blocks,
                slot as VarId,
                &SlotValue::Parameter(value),
                &function.reg_types,
            )?;
        }
        for (name, callable) in &self.callables {
            let slot = function
                .var_names
                .iter()
                .skip(function.n_params)
                .position(|var| var == name)?
                + function.n_params;
            read_binder(
                &mut function.blocks,
                slot as VarId,
                &SlotValue::Function(callable),
                &function.reg_types,
            )?;
        }
        if !self.members.is_empty() {
            read_members(&mut function.blocks, &self.members, &function.reg_types);
        }
        fold_parameter_constants(&mut function.blocks, &function.reg_types);
        self.closures
            .iter()
            .map(|(name, closure)| {
                let slot = function
                    .var_names
                    .iter()
                    .skip(function.n_params)
                    .position(|var| var == name)?
                    + function.n_params;
                Some((slot as VarId, *closure))
            })
            .collect()
    }
}

impl MutTypeVisitor for Instantiation {
    type Error = ParamError;

    fn visit_ty(&mut self, ty: &mut Ty, _: TySite) -> Result<(), ParamError> {
        *ty = self.instantiate(ty)?;
        Ok(())
    }

    fn visit_ty_arg(&mut self, arg: &mut TyArg) -> Result<(), ParamError> {
        *arg = match arg {
            TyArg::Ty(ty) => TyArg::Ty(self.instantiate(ty)?),
            _ => replace_argument_parameters(&self.context, arg, &self.bindings)?,
        };
        Ok(())
    }

    fn visit_variant_alternatives(&mut self, alternatives: &mut Vec<Ty>) -> Result<(), ParamError> {
        for ty in alternatives {
            *ty = self.instantiate(ty)?;
        }
        Ok(())
    }

    /// An index into a bound pack's alternatives closes only through the
    /// elaborator's pack logic, so one left open keeps the call.
    fn visit_variant_index(&mut self, index: &mut VariantIndex) -> Result<(), ParamError> {
        *index = index.try_map_expr(|expr| self.context.replace(expr, &self.bindings))?;
        match index {
            VariantIndex::Expr(expr)
                if matches!(expr.kind(), ParamKind::PackQuery { pack, .. }
                    if self.types.iter().any(|bound| bound.id == pack.id)) =>
            {
                Err(ParamError::Unsupported(format!(
                    "an index into the bound pack of `{expr}`"
                )))
            }
            _ => Ok(()),
        }
    }

    fn visit_lane_mask(&mut self, mask: &mut LaneMask) -> Result<(), ParamError> {
        *mask = mask.try_map_exprs(&mut |expr| self.context.replace(expr, &self.bindings))?;
        Ok(())
    }

    fn visit_param_arg(
        &mut self,
        arg: &mut MirParamArg,
        _: ParamArgSite,
    ) -> Result<(), ParamError> {
        if let Some(expr) = &mut arg.expr {
            *expr = self.context.replace(expr, &self.bindings)?;
        }
        Ok(())
    }

    /// A contract's own binders are bound by its callee, as the elaborator
    /// leaves them.
    fn visit_param_decls(
        &mut self,
        _: &mut [ParamDecl],
        _: ParamDeclSite,
    ) -> Result<(), ParamError> {
        Ok(())
    }

    /// A compile-time application over a bound type pack (`Tuple.reverse`'s
    /// reversed element list) is applied to the bound list, which the
    /// elaborator evaluates in the caller's instance.
    fn visit_constant(&mut self, constant: &mut Const) -> Result<(), ParamError> {
        match constant {
            Const::Param(expr) => *expr = self.context.replace(expr, &self.bindings)?,
            Const::Value(value) => {
                *value = replace_value_parameters(&self.context, value, &self.bindings)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// A constructed type binder renames to the binder it is bound to; at
    /// any other binding the construction is the bound type's own, which
    /// [`instantiated`] writes.
    fn visit_param_ref(&mut self, param: &mut ParamRef) -> Result<(), ParamError> {
        if self.types.contains(param)
            && let Some(TyKind::Param { binder, .. }) =
                self.bindings.types().get(&param.id).map(Ty::kind)
        {
            *param = binder.clone();
        }
        Ok(())
    }

    fn visit_constraint(&mut self, cond: &mut GenericConstraint) -> Result<(), ParamError> {
        *cond = replace_constraint_parameters(&self.context, cond, &self.bindings)?;
        Ok(())
    }

    /// A `comptime for` sequence closes over the bindings as a lane mask
    /// does; its loop binder stays the body's own, which the elaborator
    /// unrolls by identity in the caller as it would in the callee.
    fn visit_sequence(&mut self, source: &mut ComptimeSequence) -> Result<(), ParamError> {
        *source = source.try_map_exprs(&mut |expr| self.context.replace(expr, &self.bindings))?;
        Ok(())
    }
}

/// A construction of one of the callee's binders is the bound type's: the
/// binding is already in the caller's terms, so a selected pack element
/// closes as it is.
impl ConstructionTypes for Instantiation {
    type Error = TypeConstructionError;

    fn bound(&self, param: &ParamRef) -> Option<&Ty> {
        if !self.types.contains(param) {
            return None;
        }
        self.bindings
            .types()
            .get(&param.id)
            .filter(|ty| !matches!(ty.kind(), TyKind::Param { .. }))
    }

    fn element_index(&self, element: &MirParamArg) -> Option<usize> {
        element
            .expr
            .as_ref()?
            .as_constant()
            .and_then(mojito_types::param_expr::fold::integer_value)?
            .to_i64()
            .and_then(|value| usize::try_from(value).ok())
    }

    fn close(&self, ty: &Ty) -> Result<Ty, TypeConstructionError> {
        Ok(ty.clone())
    }
}

/// What a read of a binder's slot becomes.
enum SlotValue<'a> {
    /// A value binder's argument, as a constant typed by the read.
    Parameter(&'a ParamExpr),
    /// A callable binder's argument: the function its register names, as
    /// the constant upstream's substitution leaves; a call through the
    /// slot becomes a call through that constant.
    Function(&'a str),
}

impl SlotValue<'_> {
    /// The constant a read of the slot typed `ty`, through `place` when it
    /// loads one, becomes; `None` for a projection of a callable or of a
    /// value not closed enough to project.
    fn constant(&self, ty: Option<&Ty>, place: Option<&MirPlace>) -> Option<Const> {
        match (self, place) {
            (Self::Parameter(value), None) => Some(parameter_constant(value, ty)),
            (Self::Parameter(value), Some(place)) if place.is_whole_root() => {
                Some(parameter_constant(value, ty))
            }
            (Self::Parameter(value), Some(place)) => Const::of_projected_parameter_value(
                value.as_constant()?,
                &place.storage().proj,
                place.ty.as_ref(),
            ),
            (Self::Function(function), place) => place
                .is_none_or(MirPlace::is_whole_root)
                .then(|| Const::Function((*function).to_string())),
        }
    }
}

/// Rewrite each read of the binder slot `slot` in `blocks`, its regions
/// included, into `value` as a constant typed by the read's register, and
/// delete the slot's drops; `None` when the slot is used otherwise.
fn read_binder(
    blocks: &mut [MirBlock],
    slot: VarId,
    value: &SlotValue<'_>,
    reg_types: &std::collections::HashMap<u32, Ty>,
) -> Option<()> {
    for block in blocks {
        let mut kept = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            for region in instruction_regions_mut(&mut instruction) {
                read_binder(region, slot, value, reg_types)?;
            }
            let read = match &instruction {
                MirInstr::UseVar { dest, var, .. } if *var == slot => Some((*dest, None)),
                MirInstr::LoadPlace { dest, place } if place.root == slot => {
                    Some((*dest, Some(place)))
                }
                MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var } if *var == slot => {
                    continue;
                }
                _ => None,
            };
            if let Some((dest, place)) = read {
                let k = value.constant(reg_types.get(&dest.0), place)?;
                kept.push(MirInstr::Const { dest, k });
                continue;
            }
            if let (SlotValue::Function(_), MirInstr::CallIndirect { callee_place, .. }) =
                (value, &mut instruction)
                && callee_place
                    .as_deref()
                    .is_some_and(|place| place.root == slot && place.is_whole_root())
            {
                *callee_place = None;
            }
            if instruction_slots(&instruction).contains(&slot)
                || instruction_places(&instruction)
                    .iter()
                    .any(|place| place.root == slot || place.through == Some(slot))
            {
                return None;
            }
            kept.push(instruction);
        }
        block.instrs = kept;
    }
    Some(())
}

/// Rewrite each read of an owner value binder through the receiver, slot
/// 0, in `blocks`, its regions included, into the binder's argument as a
/// constant typed by the read's register, where the argument is closed
/// enough to select what the read projects.
fn read_members(
    blocks: &mut [MirBlock],
    members: &[(String, ParamExpr)],
    reg_types: &std::collections::HashMap<u32, Ty>,
) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in instruction_regions_mut(instruction) {
                read_members(region, members, reg_types);
            }
            let MirInstr::LoadPlace { dest, place } = instruction else {
                continue;
            };
            if place.root != 0 || place.through.is_some() {
                continue;
            }
            let storage = place.storage();
            let Some((Proj::Field(field), projected)) = storage.proj.split_first() else {
                continue;
            };
            let Some((_, value)) = members.iter().find(|(name, _)| name == field) else {
                continue;
            };
            let ty = reg_types.get(&dest.0);
            let k = if projected.is_empty() {
                Some(parameter_constant(value, ty))
            } else {
                value
                    .as_constant()
                    .and_then(|value| Const::of_projected_parameter_value(value, projected, ty))
            };
            if let Some(k) = k {
                *instruction = MirInstr::Const { dest: *dest, k };
            }
        }
    }
}

/// How many of `decls`, a member's binders, restate its owner's binders
/// `owner` first: all of them or none.
fn restated_owner_binders(owner: &[ParamDecl], decls: &[ParamDecl]) -> usize {
    let restated = decls.len() >= owner.len()
        && owner
            .iter()
            .zip(decls)
            .all(|(owner, decl)| owner.id() == decl.id());
    if restated { owner.len() } else { 0 }
}

/// The rewrite of a member's bare spelling of its owner, `template`, which
/// denotes the owner's own type, to the type `owner` a call instantiates.
struct BareOwner<'a> {
    template: &'a str,
    owner: &'a Ty,
}

impl TyRewrite for BareOwner<'_> {
    fn whole(&mut self, ty: &Ty) -> Option<Ty> {
        matches!(ty.kind(), TyKind::Struct(name, arguments)
            if name == self.template && arguments.is_empty())
        .then(|| self.owner.clone())
    }

    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        Ok(expr.clone())
    }

    fn visits_closed(&self) -> bool {
        true
    }
}

/// The rewrite of a dependent type the substitution selected from a pack
/// bound over the caller's binders (`Ts[0]` at `Ts = [A, B]`) to the type
/// it selected, which is the type itself.
struct SelectedShapes;

impl TyRewrite for SelectedShapes {
    fn whole(&mut self, ty: &Ty) -> Option<Ty> {
        let TyKind::Dependent(dependent) = ty.kind() else {
            return None;
        };
        let ParamKind::TypeShape(shape) = dependent.expr().kind() else {
            return None;
        };
        rewrite_ty(shape, self).ok()
    }

    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        Ok(expr.clone())
    }
}

/// Whether each of a call's capture accesses `accesses` is to a place a
/// closure the call binds a callable parameter to (`bound`, among the
/// caller's `closures`) captures: the inlined body performs it through
/// that closure's call.
fn accessed_through(
    accesses: &[MirCaptureAccess],
    bound: &[(VarId, Reg)],
    closures: &Closures,
) -> bool {
    accesses.iter().all(|access| {
        bound.iter().any(|(_, closure)| {
            closures.made.get(&closure.0).is_some_and(|closure| {
                closure
                    .captures
                    .iter()
                    .any(|capture| capture.place.root == access.root)
            })
        })
    })
}

/// Fold each `Const::Param` of `blocks` the substitution closed into the
/// constant its register reads it as.
fn fold_parameter_constants(
    blocks: &mut [MirBlock],
    reg_types: &std::collections::HashMap<u32, Ty>,
) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in instruction_regions_mut(instruction) {
                fold_parameter_constants(region, reg_types);
            }
            if let MirInstr::Const { dest, k } = instruction
                && let Const::Param(expr) = k
                && expr.as_constant().is_some()
            {
                *k = parameter_constant(expr, reg_types.get(&dest.0));
            }
        }
    }
}

/// Whether the call `instruction` passes a compile-time argument's run-time
/// register, which its inlined body, instantiated at the argument, does not
/// read.
fn passes_value_arguments(instruction: &MirInstr) -> bool {
    match instruction {
        MirInstr::Call { param_arg_regs, .. }
        | MirInstr::MethodCall { param_arg_regs, .. }
        | MirInstr::CallIndirect { param_arg_regs, .. } => param_arg_regs
            .iter()
            .any(|argument| argument.value.is_some()),
        _ => false,
    }
}

/// Whether `blocks`, regions included, still construct one of `binders`:
/// a construction the binding left open, which the caller cannot name.
fn constructs_binder(blocks: &[MirBlock], binders: &[ParamRef]) -> bool {
    blocks.iter().flat_map(|block| &block.instrs).any(|instruction| {
        matches!(instruction, MirInstr::ConstructTypeParam { param, .. } if binders.contains(param))
            || instruction_regions(instruction)
                .into_iter()
                .any(|region| constructs_binder(region, binders))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(instrs: Vec<MirInstr>, term: MirTerm) -> Vec<MirBlock> {
        vec![MirBlock { instrs, term }]
    }

    fn body(blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            blocks,
            n_regs: 1,
            ret_ty: Some(Ty::none()),
            reg_types: HashMap::from([(0, Ty::bool())]),
            ..MirFunction::default()
        }
    }

    /// A structured body prices each loop its `continue`s resume as one
    /// back edge, as upstream prices `hlcf` directly.
    #[test]
    fn structured_back_edges_count_resumed_loops() {
        let raising_body = one(
            vec![MirInstr::Loop {
                label: Some(LoopLabel(0)),
                operands: Vec::new(),
                args: Vec::new(),
                results: Vec::new(),
                body: one(
                    Vec::new(),
                    MirTerm::Raise {
                        src: Reg(0),
                        label: None,
                        values: Vec::new(),
                    },
                ),
            }],
            MirTerm::Unreachable,
        );
        let breaking_if = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: one(
                Vec::new(),
                MirTerm::Break {
                    label: Some(LoopLabel(0)),
                    values: Vec::new(),
                },
            ),
            else_args: Vec::new(),
            elifs: Vec::new(),
            orelse: one(
                Vec::new(),
                MirTerm::Break {
                    label: Some(LoopLabel(0)),
                    values: Vec::new(),
                },
            ),
        };
        let dead_continue = one(
            vec![MirInstr::Loop {
                label: Some(LoopLabel(0)),
                operands: Vec::new(),
                args: Vec::new(),
                results: Vec::new(),
                body: one(
                    vec![breaking_if],
                    MirTerm::Continue {
                        label: Some(LoopLabel(0)),
                        values: Vec::new(),
                    },
                ),
            }],
            MirTerm::Return(None),
        );
        let outer_continue = one(
            vec![MirInstr::Loop {
                label: Some(LoopLabel(0)),
                operands: Vec::new(),
                args: Vec::new(),
                results: Vec::new(),
                body: one(
                    vec![MirInstr::Loop {
                        label: Some(LoopLabel(1)),
                        operands: Vec::new(),
                        args: Vec::new(),
                        results: Vec::new(),
                        body: one(
                            vec![MirInstr::If {
                                results: Vec::new(),
                                cond: Reg(0),
                                then: one(
                                    Vec::new(),
                                    MirTerm::Continue {
                                        label: Some(LoopLabel(0)),
                                        values: Vec::new(),
                                    },
                                ),
                                else_args: Vec::new(),
                                elifs: Vec::new(),
                                orelse: one(
                                    Vec::new(),
                                    MirTerm::Break {
                                        label: Some(LoopLabel(1)),
                                        values: Vec::new(),
                                    },
                                ),
                            }],
                            MirTerm::Unreachable,
                        ),
                    }],
                    MirTerm::Break {
                        label: Some(LoopLabel(0)),
                        values: Vec::new(),
                    },
                ),
            }],
            MirTerm::Return(None),
        );
        let match_to_orelse = one(
            vec![MirInstr::Loop {
                label: Some(LoopLabel(0)),
                operands: Vec::new(),
                args: Vec::new(),
                results: Vec::new(),
                body: one(
                    vec![MirInstr::Match {
                        results: Vec::new(),
                        else_args: Vec::new(),
                        cases: vec![
                            mojito_mir::mir::MirCase {
                                args: Vec::new(),
                                body: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
                            },
                            mojito_mir::mir::MirCase {
                                args: Vec::new(),
                                body: one(Vec::new(), MirTerm::MatchNext(Vec::new())),
                            },
                        ],
                        orelse: one(Vec::new(), MirTerm::Yield(Vec::new())),
                    }],
                    MirTerm::Continue {
                        label: Some(LoopLabel(0)),
                        values: Vec::new(),
                    },
                ),
            }],
            MirTerm::Unreachable,
        );
        for (blocks, edges) in [
            (raising_body, 0),
            (dead_continue, 0),
            (outer_continue, 1),
            (match_to_orelse, 1),
        ] {
            let function = body(blocks);
            assert_eq!(structured_back_edges(&function.blocks), edges);
        }
    }
}
