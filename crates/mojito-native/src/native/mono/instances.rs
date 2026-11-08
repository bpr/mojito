//! Witness lookup, the writer-protocol test, and the constants of
//! value-parameter reads.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Specializer<'_> {
    /// The constant a value-parameter member read (`Self.length`) at `read`
    /// resolves to, when `field` names a value parameter (not a declared
    /// field) of the receiver's template and the instance type carries its
    /// solution.
    pub(super) fn value_param_constant(
        &self,
        receiver: &Ty,
        field: &str,
        read: Option<&Ty>,
    ) -> Option<Const> {
        let (name, type_args) = peel_refs(receiver).kind().as_non_simd_struct()?;
        let struct_decl = self.structs.get(nominal_template(name))?;
        if struct_decl
            .fields
            .iter()
            .any(|(field_name, _)| field_name == field)
        {
            return None;
        }
        let position = struct_decl
            .param_decls
            .iter()
            .position(|decl| matches!(decl, ParamDecl::Value { name, .. } if name == field))?;
        let TyArg::Val(value) = type_args.get(position)? else {
            return None;
        };
        match value {
            CtValue::Int(v) => Some(Const::Int(*v)),
            CtValue::UInt(v) => Some(Const::Value(CtValue::UInt(*v))),
            CtValue::Index(v) => Some(Const::Value(CtValue::Index(*v))),
            CtValue::Bool(v) => Some(Const::Bool(*v)),
            CtValue::Dtype(v) => Some(Const::Dtype(*v)),
            value @ (CtValue::Simd { .. } | CtValue::Struct { .. }) => {
                Some(Const::Value(value.clone()))
            }
            CtValue::Str(_) if read.is_some_and(mojito_types::types::is_nominal_string) => {
                Some(Const::Value(value.clone()))
            }
            _ => None,
        }
    }

    /// The member the receiver's template records in its witness table for
    /// the requirement a call through a bound selects (`dispatch`), as
    /// upstream's specialization folds `#kgen.get_witness`. A struct
    /// recording none fails as upstream's materialization does.
    pub(super) fn witness(
        &self,
        owner: &str,
        receiver_name: &str,
        dispatch: &str,
    ) -> Result<String, MonoError> {
        let template = nominal_template(receiver_name);
        self.structs
            .get(template)
            .and_then(|declaration| declaration.witness(dispatch))
            .map(str::to_string)
            .ok_or_else(|| {
                self.error(
                    Some(owner),
                    format!(
                        "struct `{template}` records no witness for the requirement `{dispatch}`"
                    ),
                )
            })
    }

    /// The `__call__` the receiver's template records for its `def(...)`
    /// conformance, which a call through a `def(...)` bound runs on a struct
    /// instance. A struct declaring none fails as [`Self::witness`] does.
    pub(super) fn callable_witness(
        &self,
        owner: &str,
        receiver_name: &str,
    ) -> Result<String, MonoError> {
        let template = nominal_template(receiver_name);
        self.structs
            .get(template)
            .and_then(|declaration| declaration.callable_witness())
            .map(str::to_string)
            .ok_or_else(|| {
                self.error(
                    Some(owner),
                    format!("struct `{template}` records no witness for a `def(...)` conformance"),
                )
            })
    }

    /// Whether the declared struct `receiver` is an instance of answers the
    /// requirement `dispatch` with a body: `SIMD`'s, for a `SIMD`
    /// receiver, once `std.simd` declares the witness. Any other requirement
    /// is the builtin's to answer.
    pub(super) fn declares_witness(&self, receiver: &Ty, dispatch: &str) -> bool {
        mojito_types::types::nominal_parts(receiver)
            .and_then(|(name, _)| self.structs.get(name))
            .and_then(|declaration| declaration.witness(dispatch))
            .is_some_and(|target| self.functions.contains_key(target))
    }

    /// Whether a display method takes the `Writable` protocol's writer; any
    /// other `write_to` is an ordinary overload and never serves display.
    pub(super) fn writes_protocol(&self, target: &str) -> bool {
        self.declarations.get(target).is_none_or(|declaration| {
            declaration
                .param_types
                .first()
                .is_some_and(mojito_types::types::is_writer_parameter)
        })
    }
}
