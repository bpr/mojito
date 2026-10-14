//! How a checked node encodes: as an MLIR bytecode op does
//! (`BytecodeWriter::writeOp`), an encoding mask naming the attributes the
//! node holds, then its always-present parts, then those attributes alone,
//! in the mask's bit order. An absent attribute writes nothing.

use super::{CheckedDeclaration, CheckedExpr, CheckedStmt, EffectFacts, ValueCategory};

masked_node! {
    CheckedExpr {
        parts {
            op,
            span,
            source: Option<String> => mojito_ast::ast::statement_module,
            syntax_id: mojito_common::token::SyntaxId => mojito_common::token::node_syntax,
        }
        packed category: ValueCategory;
        // By measured frequency in the std package, so the common seven
        // bits share the mask's first varint byte.
        attributes {
            ty,
            place_ty,
            binding,
            adjustments,
            binding_ty,
            effects,
            contextual_base,
            comprehension_bindings,
            call_transfers,
        }
    }
}

masked_node! {
    CheckedStmt {
        parts {
            op,
            span,
            module: Option<String> => mojito_ast::ast::statement_module,
            syntax_id: mojito_common::token::SyntaxId => mojito_common::token::node_syntax,
        }
        attributes {
            binding,
            declaration,
            binding_ty,
        }
    }
}

masked_node! {
    CheckedDeclaration {
        parts {
            kind,
            name,
            location,
        }
        attributes {
            binding,
            children,
            ty,
        }
    }
}

/// A node attribute the encoding mask can name absent.
trait Attribute: Default {
    type Value: serde::Serialize + serde::de::DeserializeOwned;

    /// What the attribute writes, when it is present.
    fn value(&self) -> Option<&Self::Value>;

    fn from_value(value: Self::Value) -> Self;
}

impl<T: serde::Serialize + serde::de::DeserializeOwned> Attribute for Option<T> {
    type Value = T;

    fn value(&self) -> Option<&T> {
        self.as_ref()
    }

    fn from_value(value: T) -> Self {
        Some(value)
    }
}

impl<T: serde::Serialize + serde::de::DeserializeOwned> Attribute for Vec<T> {
    type Value = Self;

    fn value(&self) -> Option<&Self> {
        (!self.is_empty()).then_some(self)
    }

    fn from_value(value: Self) -> Self {
        value
    }
}

impl Attribute for EffectFacts {
    type Value = Self;

    fn value(&self) -> Option<&Self> {
        (*self != Self::default()).then_some(self)
    }

    fn from_value(value: Self) -> Self {
        value
    }
}

/// A node part the encoding mask carries in its low bits.
trait Packed: Sized {
    const BITS: u32;

    fn code(self) -> u16;

    fn from_code(code: u16) -> Option<Self>;
}

impl Packed for ValueCategory {
    const BITS: u32 = 2;

    fn code(self) -> u16 {
        match self {
            Self::Value => 0,
            Self::Place => 1,
            Self::Type => 2,
            Self::CompileTime => 3,
        }
    }

    fn from_code(code: u16) -> Option<Self> {
        match code {
            0 => Some(Self::Value),
            1 => Some(Self::Place),
            2 => Some(Self::Type),
            3 => Some(Self::CompileTime),
            _ => None,
        }
    }
}

/// The `Serialize` and `Deserialize` of a node that encodes as an encoding
/// mask, its parts, and the attributes the mask names.
macro_rules! masked_node {
    (
        $node:ident {
            parts { $($part:ident $(: $part_ty:ty => $codec:path)?,)* }
            $(packed $packed:ident: $packed_ty:ty;)?
            attributes { $($attribute:ident,)* }
        }
    ) => {
        const _: () = assert!(
            masked_node!(@bits $($packed_ty)?) + [$(stringify!($attribute)),*].len() as u32
                <= u16::BITS,
            "the encoding mask is a u16",
        );

        impl serde::Serialize for $node {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeTuple;
                let present = [$(Attribute::value(&self.$attribute).is_some()),*];
                let mask = present.iter().enumerate().fold(
                    masked_node!(@code self $($packed)?),
                    |mask, (index, &present)| {
                        mask | u16::from(present) << (masked_node!(@bits $($packed_ty)?) as usize + index)
                    },
                );
                let parts = [$(stringify!($part)),*].len();
                let present = present.iter().filter(|&&present| present).count();
                let mut tuple = serializer.serialize_tuple(1 + parts + present)?;
                tuple.serialize_element(&mask)?;
                $(masked_node!(@encode tuple, self.$part $(, $part_ty, $codec)?);)*
                $(
                    if let Some(value) = Attribute::value(&self.$attribute) {
                        tuple.serialize_element(value)?;
                    }
                )*
                tuple.end()
            }
        }

        impl<'de> serde::Deserialize<'de> for $node {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Visitor;

                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = $node;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                        formatter.write_str(concat!("a masked ", stringify!($node)))
                    }

                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<$node, A::Error> {
                        let mut read = 0;
                        let mut next = || {
                            read += 1;
                            read
                        };
                        let mask: u16 = seq
                            .next_element()?
                            .ok_or_else(|| serde::de::Error::invalid_length(next(), &self))?;
                        let bits = masked_node!(@bits $($packed_ty)?);
                        let attributes = [$(stringify!($attribute)),*].len() as u32;
                        if u32::from(mask).checked_shr(bits + attributes).unwrap_or(0) != 0 {
                            return Err(serde::de::Error::custom(format!(
                                concat!("a ", stringify!($node), " mask names unknown attributes: {:#x}"),
                                mask
                            )));
                        }
                        $(
                            let $part = masked_node!(@decode seq $(, $part_ty, $codec)?)
                                .ok_or_else(|| serde::de::Error::invalid_length(next(), &self))?;
                        )*
                        $(
                            let $packed = <$packed_ty as Packed>::from_code(
                                mask & ((1 << <$packed_ty as Packed>::BITS) - 1),
                            )
                            .ok_or_else(|| serde::de::Error::custom("an unknown packed code"))?;
                        )?
                        let mut named = (bits..).map(|bit| mask & (1 << bit) != 0);
                        $(
                            let $attribute = if named.next() == Some(true) {
                                Attribute::from_value(
                                    seq.next_element()?.ok_or_else(|| {
                                        serde::de::Error::invalid_length(next(), &self)
                                    })?,
                                )
                            } else {
                                Default::default()
                            };
                        )*
                        Ok($node {
                            id: Default::default(),
                            $($part,)*
                            $($packed,)?
                            $($attribute,)*
                        })
                    }
                }

                let parts = [$(stringify!($part)),*].len();
                let attributes = [$(stringify!($attribute)),*].len();
                deserializer.deserialize_tuple(1 + parts + attributes, Visitor)
            }
        }
    };
    (@bits) => { 0 };
    (@bits $packed_ty:ty) => { <$packed_ty as Packed>::BITS };
    (@code $node:ident) => { 0 };
    (@code $node:ident $packed:ident) => { Packed::code($node.$packed) };
    (@encode $tuple:ident, $value:expr) => {
        $tuple.serialize_element(&$value)?
    };
    (@encode $tuple:ident, $value:expr, $part_ty:ty, $codec:path) => {{
        struct Part<'a>(&'a $part_ty);

        impl serde::Serialize for Part<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use $codec as codec;
                codec::serialize(self.0, serializer)
            }
        }

        $tuple.serialize_element(&Part(&$value))?
    }};
    (@decode $seq:ident) => {
        $seq.next_element()?
    };
    (@decode $seq:ident, $part_ty:ty, $codec:path) => {{
        struct Part($part_ty);

        impl<'de> serde::Deserialize<'de> for Part {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                use $codec as codec;
                codec::deserialize(deserializer).map(Self)
            }
        }

        $seq.next_element::<Part>()?.map(|part| part.0)
    }};
}

use masked_node;

#[cfg(test)]
mod tests {
    use super::super::{CheckedOp, SemanticAdjustment};
    use super::*;
    use mojito_types::origin::OwnerId;
    use mojito_types::types::Ty;

    fn node() -> CheckedExpr {
        CheckedExpr {
            id: super::super::CheckedNodeId::default(),
            op: CheckedOp::Bool(true),
            span: (3, 7),
            source: None,
            syntax_id: mojito_common::token::SyntaxId(0),
            ty: Some(Ty::int()),
            place_ty: None,
            binding_ty: None,
            category: ValueCategory::Place,
            binding: None,
            contextual_base: None,
            effects: EffectFacts::default(),
            adjustments: Vec::new(),
            comprehension_bindings: Vec::new(),
            call_transfers: Vec::new(),
        }
    }

    fn encode(node: &CheckedExpr) -> Vec<u8> {
        mojito_types::origin::encoding_packaged_owners(|| postcard::to_stdvec(node))
            .expect("encodes")
    }

    fn decode(bytes: &[u8]) -> Result<CheckedExpr, postcard::Error> {
        mojito_types::origin::encoding_packaged_owners(|| postcard::from_bytes(bytes))
    }

    fn assert_round_trips(node: &CheckedExpr) -> Vec<u8> {
        let bytes = encode(node);
        // Outside an occurrence encoding a node draws a fresh syntax id.
        let decoded = CheckedExpr {
            syntax_id: node.syntax_id,
            ..decode(&bytes).expect("decodes")
        };
        assert_eq!(format!("{decoded:?}"), format!("{node:?}"));
        bytes
    }

    #[test]
    fn a_node_writes_only_the_attributes_it_has() {
        let bytes = assert_round_trips(&node());
        // Category `Place` (1) in bits 0-1, `ty` in bit 2.
        assert_eq!(bytes[0], 0b101, "a one-byte mask");
        let mut bare = node();
        bare.ty = None;
        let ty = postcard::to_stdvec(&Ty::int()).expect("encodes");
        assert_eq!(encode(&bare).len() + ty.len(), bytes.len());
    }

    #[test]
    fn a_node_with_every_attribute_round_trips() {
        let node = CheckedExpr {
            place_ty: Some(Ty::bool()),
            binding_ty: Some(Ty::int()),
            category: ValueCategory::CompileTime,
            binding: Some(OwnerId(4)),
            contextual_base: Some("Int".to_string()),
            effects: EffectFacts {
                raises: None,
                may_suspend: true,
                diverges: false,
            },
            adjustments: vec![SemanticAdjustment::ResolveCallable("f".to_string())],
            ..node()
        };
        let bytes = assert_round_trips(&node);
        assert!(bytes[0] & 0x80 != 0, "a two-byte mask");
    }

    #[test]
    fn a_truncated_node_does_not_decode() {
        let bytes = encode(&node());
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(
            decode(&[0xff, 0xff, 0x03]).is_err(),
            "unknown attribute bits"
        );
    }
}
