//! Parameter uses: which of a generic function's own compile-time
//! parameters its body and signature read, the analysis upstream's
//! `RemoveUnusedParams` runs before it drops a parameter from a generator.
//!
//! MIR names a binder by its identity wherever it refers to one: a type
//! parameter, a dependent type, a parameter expression, a call's parameter
//! arguments. So the walk is structural, after MLIR's walk over every
//! attribute and type an operation holds: it visits the value's serialized
//! form, and a `(owner, slot)` pair spelling one of the function's binders
//! is a read of it. A pair that only happens to spell one counts too, which
//! keeps a parameter and never drops one. Two further reads are not
//! identities: a value parameter's runtime reads go through the slot its
//! name binds, and a body nested in or lifted from the function (named
//! `f$…`, `$comptime$f$…`, `$default$f$…`) reads whatever of the function's
//! scope it likes, so naming one reads every parameter.

use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirTerm,
    instruction_regions, instruction_slots,
};
use mojito_hir::hir::VarId;
use mojito_types::param_expr::ParamId;
use mojito_types::types::{ParamDecl, Ty, TyKind};
use serde::Serialize;
use serde::ser;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

/// The parameters upstream's `RemoveUnusedParams` removes from `name`.
///
/// They are its own compile-time parameters that nothing in its body
/// `function` or its declaration's signature reads, sorted by slot.
///
/// A default of another parameter is not a read, since a default is the
/// call site's. A nested function, `__toplevel__`, and a function without a
/// declaration have no candidates, nor has a callable value parameter,
/// whose binding can add runtime parameters to the instance.
pub fn unread_parameters(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<ParamId> {
    let Some(declaration) = declarations.function(name) else {
        return Vec::new();
    };
    if name == "__toplevel__"
        || declaration.enclosing.is_some()
        || !declaration.region_binders.is_empty()
    {
        return Vec::new();
    }
    let candidates: Vec<&ParamDecl> = declaration
        .param_decls
        .iter()
        .filter(|decl| &*decl.id().owner == name && !is_callable_value(decl))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut walker = Walker::new(name);
    walker.signature(declaration);
    walker.body(function);
    if walker.reads_all {
        return Vec::new();
    }
    let read_by_slot = slot_reads(function, &candidates);
    let mut unread: Vec<ParamId> = candidates
        .into_iter()
        .filter(|decl| !walker.slots.contains(&decl.id().slot))
        .filter(|decl| !read_by_slot.contains(decl.name().trim_start_matches('*')))
        .map(|decl| decl.id().clone())
        .collect();
    unread.sort_by_key(|id| id.slot);
    unread
}

fn is_callable_value(decl: &ParamDecl) -> bool {
    matches!(
        decl,
        ParamDecl::Value { ty, callable_default, .. }
            if callable_default.is_some()
                || matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
    )
}

/// The names of the value parameters among `candidates` whose slot the body
/// reads: any instruction naming the slot, but a drop or a declaration of a
/// slot whose type has no destructor to run.
fn slot_reads(function: &MirFunction, candidates: &[&ParamDecl]) -> HashSet<String> {
    let names: HashSet<&str> = candidates
        .iter()
        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
        .map(|decl| decl.name().trim_start_matches('*'))
        .collect();
    let slots: HashMap<VarId, &str> = function
        .var_names
        .iter()
        .enumerate()
        .filter_map(|(slot, name)| names.get(name.as_str()).map(|name| (slot as VarId, *name)))
        .collect();
    let mut reads = HashSet::new();
    if !slots.is_empty() {
        let mut cx = SlotReads {
            function,
            slots: &slots,
            reads: &mut reads,
        };
        cx.blocks(&function.blocks);
    }
    reads
}

struct SlotReads<'a> {
    function: &'a MirFunction,
    slots: &'a HashMap<VarId, &'a str>,
    reads: &'a mut HashSet<String>,
}

impl SlotReads<'_> {
    fn blocks(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                self.instruction(instruction);
                for region in instruction_regions(instruction) {
                    self.blocks(region);
                }
            }
            if let MirTerm::ReturnWithCleanup { cleanup, .. }
            | MirTerm::EscapeJump { cleanup, .. } = &block.term
            {
                self.drops(cleanup);
            }
        }
    }

    fn instruction(&mut self, instruction: &MirInstr) {
        match instruction {
            MirInstr::DropVar { var }
            | MirInstr::ConsumeVar { var }
            | MirInstr::DeclareVar { var } => {
                self.drops(std::slice::from_ref(var));
            }
            MirInstr::Try {
                handler, cleanup, ..
            } => {
                if let Some((Some(binding), _)) = handler {
                    self.read(*binding);
                }
                self.drops(cleanup);
            }
            _ => {
                for slot in instruction_slots(instruction) {
                    self.read(slot);
                }
            }
        }
    }

    /// Dropping a slot reads its value only when a destructor runs on it.
    fn drops(&mut self, slots: &[VarId]) {
        for &slot in slots {
            if !self
                .function
                .var_tys
                .get(&slot)
                .is_some_and(has_no_destructor)
            {
                self.read(slot);
            }
        }
    }

    fn read(&mut self, slot: VarId) {
        if let Some(name) = self.slots.get(&slot) {
            self.reads.insert((*name).to_string());
        }
    }
}

/// A compiler-known scalar, whose drop runs no code.
fn has_no_destructor(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Int
            | TyKind::UInt
            | TyKind::Bool
            | TyKind::StringLiteral
            | TyKind::None
            | TyKind::IntLiteral
            | TyKind::FloatLiteral
            | TyKind::Dtype
            | TyKind::Simd { .. }
    )
}

/// The serializer [`unread_parameters`] walks a value with, looking for the
/// identities of the binders of `owner`.
struct Walker<'a> {
    owner: &'a str,
    /// The slots of `owner`'s binders a walked value names.
    slots: BTreeSet<usize>,
    /// Whether a walked value names a body nested in or lifted from
    /// `owner`, or holds a leaf no walk can see into, so that every
    /// parameter counts as read.
    reads_all: bool,
    /// The last leaf a walk visited.
    leaf: Leaf,
    /// Each tuple the walk is inside: its length, how many of its elements
    /// were walked, and whether its first was `owner`.
    tuples: Vec<(usize, usize, bool)>,
    /// The slots each type walked so far names: a type is a uniqued handle
    /// that many registers and slots share.
    types: HashMap<Ty, (BTreeSet<usize>, bool)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Other,
    Owner,
    Slot(u64),
}

impl<'a> Walker<'a> {
    fn new(owner: &'a str) -> Self {
        Self {
            owner,
            slots: BTreeSet::new(),
            reads_all: false,
            leaf: Leaf::Other,
            tuples: Vec::new(),
            types: HashMap::new(),
        }
    }

    /// The declaration's signature: its runtime parameters, collectors,
    /// result, error, runtime defaults, and `where` clauses, and each
    /// compile-time parameter's type and constraints. Not a compile-time
    /// parameter's default, which the call site evaluates.
    fn signature(&mut self, declaration: &MirFunctionDeclaration) {
        for ty in declaration
            .param_types
            .iter()
            .chain(&declaration.variadic)
            .chain(&declaration.kw_variadic)
            .chain(std::iter::once(&declaration.ret_ty))
            .chain(&declaration.error_ty)
        {
            self.ty(ty);
        }
        self.visit(&declaration.defaults);
        self.visit(&declaration.availability);
        for decl in &declaration.param_decls {
            match decl {
                ParamDecl::Type {
                    callable_bound,
                    constraints,
                    ..
                } => {
                    if let Some(bound) = callable_bound {
                        self.ty(bound);
                    }
                    self.visit(constraints);
                }
                ParamDecl::Value {
                    ty, constraints, ..
                } => {
                    self.ty(ty);
                    self.visit(constraints);
                }
            }
        }
    }

    /// The body: its blocks, and the types of its signature, slots, and
    /// registers.
    fn body(&mut self, function: &MirFunction) {
        self.visit(&function.blocks);
        for ty in function
            .param_types
            .iter()
            .chain(function.var_tys.values())
            .chain(&function.ret_ty)
            .chain(&function.error_ty)
            .chain(function.reg_types.values())
        {
            self.ty(ty);
        }
    }

    fn ty(&mut self, ty: &Ty) {
        if !self.types.contains_key(ty) {
            let mut walker = Walker::new(self.owner);
            walker.visit(ty);
            self.types
                .insert(ty.clone(), (walker.slots, walker.reads_all));
        }
        let (slots, reads_all) = &self.types[ty];
        self.slots.extend(slots);
        self.reads_all |= reads_all;
    }

    /// Walk `value`. A leaf that encodes only in its compilation's context
    /// (a binding owner, a source location) names no binder, and its
    /// siblings are still walked; a value serde cannot encode at all (an
    /// unbound parameter hole) may name anything.
    fn visit<T: Serialize + ?Sized>(&mut self, value: &T) {
        let depth = self.tuples.len();
        if let Err(Unwalkable(opaque)) = value.serialize(&mut *self) {
            self.reads_all |= opaque;
        }
        self.tuples.truncate(depth);
    }

    fn string(&mut self, value: &str) {
        let owned = |rest: Option<&str>| {
            rest.and_then(|rest| rest.strip_prefix(self.owner))
                .is_some_and(|rest| rest.starts_with('$'))
        };
        if owned(Some(value))
            || owned(value.strip_prefix("$comptime$"))
            || owned(value.strip_prefix("$default$"))
        {
            self.reads_all = true;
        }
        self.leaf = if value == self.owner {
            Leaf::Owner
        } else {
            Leaf::Other
        };
    }

    fn element<T: Serialize + ?Sized>(&mut self, value: &T) {
        self.leaf = Leaf::Other;
        self.visit(value);
        if let Some((length, index, owner)) = self.tuples.last_mut() {
            if *length == 2 {
                match (*index, self.leaf) {
                    (0, Leaf::Owner) => *owner = true,
                    (1, Leaf::Slot(slot)) if *owner => {
                        self.slots.insert(slot as usize);
                    }
                    _ => {}
                }
            }
            *index += 1;
        }
    }

    const fn compound(&mut self) -> &mut Self {
        self.leaf = Leaf::Other;
        self
    }
}

/// A value the walk could not encode: `true` when no context would encode
/// it, `false` for a leaf only its compilation's context encodes.
#[derive(Debug)]
struct Unwalkable(bool);

impl fmt::Display for Unwalkable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a value the parameter-use walk cannot encode")
    }
}

impl std::error::Error for Unwalkable {}

impl ser::Error for Unwalkable {
    fn custom<T: fmt::Display>(message: T) -> Self {
        // serde's own refusal of a skipped variant, as against a leaf's
        // refusal outside its context.
        Self(message.to_string().contains("cannot be serialized"))
    }
}

impl ser::Serializer for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_str(self, value: &str) -> Result<(), Unwalkable> {
        self.string(value);
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Slot(value);
        Ok(())
    }

    fn serialize_bool(self, _: bool) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i8(self, _: i8) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i16(self, _: i16) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i32(self, _: i32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i64(self, _: i64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i128(self, _: i128) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u8(self, _: u8) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u16(self, _: u16) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u32(self, _: u32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u128(self, _: u128) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_f32(self, _: f32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_f64(self, _: f64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_char(self, _: char) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_tuple(self, length: usize) -> Result<Self, Unwalkable> {
        self.tuples.push((length, 0, false));
        Ok(self.compound())
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }
}

impl ser::SerializeSeq for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.element(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.tuples.pop();
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeMap for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Unwalkable> {
        self.visit(key);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Walker<'_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}
