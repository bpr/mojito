//! Symbol uses: which of a program's symbols a MIR value names, after
//! MLIR's `SymbolTable::getSymbolUses`, which walks every attribute and type
//! an operation holds, nested, for symbol references.
//!
//! MIR names a function or a struct by its spelling wherever it refers to
//! one: a call's callee, a closure's function, a compile-time application,
//! a default's evaluator, a struct type. So the walk is structural: it
//! visits every string in the value's serialized form, and a string that
//! spells a symbol of the table is a use of it. A string that only happens
//! to spell a symbol counts too, which keeps a symbol live and never drops
//! one. A leaf whose encoding needs a compilation's context (a check-local
//! binding owner, a source location) names no symbol and is skipped.

use crate::mir::MirFunction;
use mojito_types::types::Ty;
use serde::Serialize;
use serde::ser;
use std::collections::{HashMap, HashSet};
use std::fmt;

/// A program's symbols, by name, and the uses a walk finds of them: MLIR's
/// `SymbolTable` over a MIR program.
pub struct SymbolTable<'t> {
    names: HashSet<&'t str>,
    /// Bit `n` is set when some name is `n` bytes long, bit 127 for every
    /// longer one, so most strings are rejected before they are hashed.
    lengths: u128,
    /// The uses each type walked so far holds: a type is a uniqued handle
    /// that many registers and slots share.
    types: HashMap<Ty, Vec<&'t str>>,
}

impl<'t> SymbolTable<'t> {
    pub fn new(names: impl IntoIterator<Item = &'t str>) -> Self {
        let names: HashSet<_> = names.into_iter().collect();
        let lengths = names
            .iter()
            .fold(0, |lengths, name| lengths | length_bit(name));
        Self {
            names,
            lengths,
            types: HashMap::new(),
        }
    }

    /// The symbols `value` names anywhere in its structure, each once per
    /// occurrence.
    pub fn uses<T: Serialize + ?Sized>(&self, value: &T) -> Vec<&'t str> {
        let mut walker = Walker {
            table: self,
            uses: Vec::new(),
        };
        walker.visit(value);
        walker.uses
    }

    /// The symbols the body `function` names: in its instructions and
    /// terminators, and in the types of its signature, slots, and
    /// registers.
    pub fn function_uses(&mut self, function: &MirFunction) -> Vec<&'t str> {
        let MirFunction {
            blocks,
            param_types,
            var_tys,
            ret_ty,
            error_ty,
            reg_types,
            // Counts, local spellings, conventions, and locations name no
            // symbol.
            n_regs: _,
            n_vars: _,
            var_names: _,
            n_params: _,
            owned_params: _,
            deinit_params: _,
            ref_params: _,
            returns_reference: _,
            raises: _,
            spans: _,
            // The function's own binders name no symbol.
            removed_params: _,
        } = function;
        let mut uses = self.uses(blocks);
        let types = param_types
            .iter()
            .chain(var_tys.values())
            .chain(ret_ty)
            .chain(error_ty)
            .chain(reg_types.values());
        for ty in types {
            uses.extend(self.type_uses(ty));
        }
        uses
    }

    /// The symbols the type `ty` names, walked once per table.
    fn type_uses(&mut self, ty: &Ty) -> &[&'t str] {
        if !self.types.contains_key(ty) {
            let uses = self.uses(ty);
            self.types.insert(ty.clone(), uses);
        }
        &self.types[ty]
    }

    fn lookup(&self, name: &str) -> Option<&'t str> {
        if self.lengths & length_bit(name) == 0 {
            return None;
        }
        self.names.get(name).copied()
    }
}

/// The bit of [`SymbolTable::lengths`] a name of `name`'s length sets.
const fn length_bit(name: &str) -> u128 {
    let length = if name.len() < 127 { name.len() } else { 127 };
    1 << length
}

/// The serializer [`SymbolTable::uses`] walks a value with: every string is
/// looked up, everything else is passed over.
struct Walker<'w, 't> {
    table: &'w SymbolTable<'t>,
    uses: Vec<&'t str>,
}

impl Walker<'_, '_> {
    /// Walk `value`, keeping the uses found before a leaf that cannot be
    /// encoded outside its context stopped it.
    fn visit<T: Serialize + ?Sized>(&mut self, value: &T) {
        // A leaf that fails names no symbol; its siblings are still walked.
        let _ = value.serialize(&mut *self);
    }
}

/// A leaf that cannot be encoded outside its compilation's context.
#[derive(Debug)]
struct Skipped;

impl fmt::Display for Skipped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a leaf encoded only in context")
    }
}

impl std::error::Error for Skipped {}

impl ser::Error for Skipped {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self
    }
}

impl ser::Serializer for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_str(self, value: &str) -> Result<(), Skipped> {
        self.uses.extend(self.table.lookup(value));
        Ok(())
    }

    fn serialize_bool(self, _: bool) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_i8(self, _: i8) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_i16(self, _: i16) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_i32(self, _: i32) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_i64(self, _: i64) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_i128(self, _: i128) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_u8(self, _: u8) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_u16(self, _: u16) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_u32(self, _: u32) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_u64(self, _: u64) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_u128(self, _: u128) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_f32(self, _: f32) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_f64(self, _: f64) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_char(self, _: char) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn serialize_unit(self) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Skipped> {
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Skipped> {
        Ok(self)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self, Skipped> {
        Ok(self)
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Skipped> {
        Ok(self)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Skipped> {
        Ok(self)
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self, Skipped> {
        Ok(self)
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Skipped> {
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Skipped> {
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

impl ser::SerializeMap for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Skipped> {
        self.visit(key);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Walker<'_, '_> {
    type Ok = ();
    type Error = Skipped;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Skipped> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Skipped> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SymbolTable;
    use crate::mir::{Const, FuncRef, MirBlock, MirFunction, MirInstr, MirTerm, Reg};
    use mojito_types::types::{Ty, TyKind};
    use std::collections::HashSet;

    fn body(instrs: Vec<MirInstr>) -> MirFunction {
        MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(None),
            }],
            ..MirFunction::default()
        }
    }

    fn call(name: &str) -> MirInstr {
        MirInstr::Call {
            dest: Reg(0),
            func: FuncRef::named(name),
            raises: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        }
    }

    #[test]
    fn calls_closures_and_function_constants_are_uses() {
        let mut table = SymbolTable::new(["f", "g", "h", "unused"]);
        let function = body(vec![
            call("f"),
            MirInstr::MakeClosure {
                dest: Reg(1),
                function: "g".to_string(),
                captures: Vec::new(),
            },
            MirInstr::Const {
                dest: Reg(2),
                k: Const::Function("h".to_string()),
            },
            call("not_in_the_table"),
        ]);
        let uses: HashSet<_> = table.function_uses(&function).into_iter().collect();
        assert_eq!(uses, HashSet::from(["f", "g", "h"]));
    }

    #[test]
    fn a_struct_named_only_in_a_try_handler_register_type_is_a_use() {
        let mut table = SymbolTable::new(["Box", "Other"]);
        let mut function = body(vec![MirInstr::Try {
            body: vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::TryYield,
            }],
            handler: Some((
                None,
                vec![MirBlock {
                    instrs: vec![MirInstr::ConstructType {
                        dest: Reg(0),
                        ty: Ty::new(TyKind::Struct("Box".to_string(), Vec::new())),
                    }],
                    term: MirTerm::TryYield,
                }],
            )),
            orelse: None,
            finalbody: None,
            cleanup: Vec::new(),
        }]);
        function.n_regs = 1;
        assert_eq!(table.function_uses(&function), vec!["Box"]);
    }
}
