//! Symbol references: the strings that name a program symbol, a function or
//! a struct, after MLIR's `SymbolRefAttr`.
//!
//! A field that refers to a symbol keeps its spelling but encodes it as a
//! [`SymbolRef`], a newtype every encoding Mojito uses passes through, so a
//! package's bytes are those of the bare string. A walk over a value's
//! encoding (`mir::SymbolTable`) tells a reference from a local, field, or
//! literal spelling by the newtype's name, as MLIR's `getSymbolUses` visits
//! only `SymbolRefAttr`s.

use serde::{Serialize, Serializer};

/// The newtype name a symbol reference encodes under.
pub const SYMBOL_REF: &str = "SymbolRef";

/// A reference to the symbol `0`, encoded as the newtype [`SYMBOL_REF`].
pub struct SymbolRef<'a>(pub &'a str);

impl Serialize for SymbolRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(SYMBOL_REF, self.0)
    }
}

/// Encode the symbol name `name` as a reference: a field's
/// `serialize_with`.
pub fn serialize<S: Serializer>(name: &str, serializer: S) -> Result<S::Ok, S::Error> {
    SymbolRef(name).serialize(serializer)
}

/// Encode an optional symbol name as a reference.
pub fn serialize_option<S: Serializer>(
    name: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    name.as_deref().map(SymbolRef).serialize(serializer)
}

/// Encode a list of symbol names as references.
pub fn serialize_seq<S: Serializer>(names: &[String], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(names.iter().map(|name| SymbolRef(name)))
}

/// Encode a list of optional symbol names as references.
pub fn serialize_option_seq<S: Serializer>(
    names: &[Option<String>],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(names.iter().map(|name| name.as_deref().map(SymbolRef)))
}
