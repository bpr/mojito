use super::*;
use crate::origin::CallableEnvironment;
use crate::types::{DependentType, TransferEffect, TransferSet, TyArg};
use std::collections::hash_map::DefaultHasher;

fn int_param(context: &ParamContext, owner: &str, slot: usize, name: &str) -> ParamExpr {
    context.register(ParamId::new(owner, slot), name, MetaTy::int())
}

fn literal(context: &ParamContext, value: i64) -> ParamExpr {
    context
        .constant(CtValue::IntLiteral(IntLiteral::from(value)))
        .expect("an integer literal is a constant")
}

fn infix(context: &ParamContext, op: InfixOp, left: &ParamExpr, right: &ParamExpr) -> ParamExpr {
    context
        .infix(op, left, right)
        .expect("well-typed operands build")
}

fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn callable(transfers: TransferSet) -> Ty {
    Ty::Func {
        environment: CallableEnvironment::default(),
        params: vec![Ty::Int],
        names: vec!["x".to_string()],
        ret: Box::new(Ty::Int),
        required: vec![true],
        variadic: None,
        kw_variadic: None,
        positional_only: None,
        keyword_only: None,
        raises: false,
        error: None,
        conventions: vec![None],
        ref_params: Box::new(vec![None]),
        ref_return: None,
        transfers,
    }
}

fn one_transfer() -> TransferSet {
    use crate::origin::SigOrigin;
    TransferSet(vec![TransferEffect {
        dest: SigOrigin::Self_,
        src: SigOrigin::Param(0),
        src_is_place: true,
        mutable: false,
    }])
}

/// The identities the pinned Mojo accepts (`assets/ok/
/// param_expr_normal_form.mojo` and the battery recorded in
/// `docs/notes/param-expr-attributes.md`) are node identity, and the ones it
/// rejects stay distinct.
#[test]
fn param_expr_canonicalization_contract() {
    let context = ParamContext::new();
    let n = int_param(&context, "f", 0, "n");
    let m = int_param(&context, "f", 1, "m");
    let lit = |value| literal(&context, value);
    let op = |op, left: &ParamExpr, right: &ParamExpr| infix(&context, op, left, right);
    use InfixOp::{Add, FloorDiv, Mul, Pow, Shl, Sub};

    // Reordered, distributed, collected.
    assert_eq!(op(Add, &n, &lit(1)), op(Add, &lit(1), &n));
    assert_eq!(
        op(Mul, &op(Add, &n, &lit(1)), &lit(4)),
        op(Add, &op(Mul, &n, &lit(4)), &lit(4))
    );
    assert_eq!(op(Add, &n, &n), op(Mul, &lit(2), &n));
    // Neutral elements, cancellation, commutativity, associativity.
    assert_eq!(op(Add, &op(Sub, &n, &lit(1)), &lit(1)), n);
    assert_eq!(op(Add, &n, &lit(0)), n);
    assert_eq!(op(Mul, &n, &lit(1)), n);
    assert_eq!(
        op(Sub, &n, &n).as_constant(),
        Some(&CtValue::Int(0)),
        "n - n folds to the machine zero"
    );
    assert_eq!(op(Sub, &op(Mul, &lit(2), &n), &n), n);
    assert_eq!(op(Mul, &n, &m), op(Mul, &m, &n));
    assert_eq!(op(Sub, &op(Add, &n, &m), &m), n);
    assert_eq!(op(Mul, &op(Mul, &n, &n), &n), op(Mul, &n, &op(Mul, &n, &n)));
    assert_eq!(op(Mul, &lit(0), &n).as_constant(), Some(&CtValue::Int(0)));
    assert_eq!(
        op(Mul, &op(Add, &n, &lit(1)), &op(Sub, &n, &lit(1))),
        op(Sub, &op(Mul, &n, &n), &lit(1))
    );
    // A left shift by a constant is a multiplication.
    assert_eq!(op(Shl, &n, &lit(1)), op(Mul, &lit(2), &n));

    // Unary negation, `//`, and `**` are opaque atoms: the pin rejects each
    // of these equalities, so they must stay distinct nodes.
    let neg_n = context.neg(&n).expect("negation builds");
    assert_ne!(op(Add, &neg_n, &n).as_constant(), Some(&CtValue::Int(0)));
    assert_ne!(op(Sub, &lit(0), &n), neg_n);
    assert_ne!(neg_n, op(Mul, &lit(-1), &n));
    assert_ne!(context.neg(&neg_n).expect("negation builds"), n);
    assert_ne!(op(FloorDiv, &n, &lit(1)), n);
    assert_ne!(op(Pow, &n, &lit(2)), op(Mul, &n, &n));
    assert_ne!(op(Add, &n, &lit(1)), op(Add, &n, &lit(2)));

    // Same spelling, unrelated binders.
    let other_n = int_param(&context, "g", 0, "n");
    assert_ne!(n, other_n);
    assert_ne!(op(Add, &n, &lit(1)), op(Add, &other_n, &lit(1)));

    // Canonicalization is idempotent under re-interning and replacement.
    let expression = op(Mul, &op(Add, &n, &lit(1)), &op(Add, &m, &lit(2)));
    assert_eq!(context.intern(&expression), expression);
    assert_eq!(
        context
            .replace(&expression, &ParamBindings::new())
            .expect("an empty replacement is the identity"),
        expression
    );
}

#[test]
fn equal_graphs_built_in_different_orders_agree_across_contexts() {
    let build = |reverse: bool| {
        let context = ParamContext::new();
        let (first, second) = if reverse { ("m", "n") } else { ("n", "m") };
        // Allocation order differs between the two contexts.
        let a = int_param(&context, "f", usize::from(first == "m"), first);
        let b = int_param(&context, "f", usize::from(second == "m"), second);
        let sum = infix(&context, InfixOp::Add, &a, &b);
        infix(&context, InfixOp::Mul, &sum, &literal(&context, 3))
    };
    let (left, right) = (build(false), build(true));
    assert_eq!(left, right);
    assert_eq!(hash_of(&left), hash_of(&right));
    assert_eq!(left.to_string(), right.to_string());
    assert_eq!(left.to_string(), "3 * n + 3 * m");

    // A same-spelled free binder of another declaration is not merged in.
    let foreign = ParamContext::new();
    let other = int_param(&foreign, "g", 0, "n");
    let imported = ParamContext::new().intern(&other);
    assert_eq!(imported, other);
    assert_ne!(imported, int_param(&foreign, "f", 0, "n"));
}

#[test]
fn replacement_folds_and_is_order_independent() {
    let context = ParamContext::new();
    let n = int_param(&context, "f", 0, "n");
    let m = int_param(&context, "f", 1, "m");
    let expression = infix(
        &context,
        InfixOp::Mul,
        &infix(&context, InfixOp::Add, &n, &literal(&context, 1)),
        &m,
    );
    let bind = |pairs: &[(&ParamExpr, i64)]| {
        let mut bindings = ParamBindings::new();
        for (parameter, value) in pairs {
            let id = parameter.as_decl_ref().expect("a reference").id.clone();
            bindings.bind(id, literal(&context, *value));
        }
        bindings
    };
    let both = context
        .replace(&expression, &bind(&[(&n, 3), (&m, 5)]))
        .expect("replacement");
    assert_eq!(both.as_constant(), Some(&CtValue::Int(20)));
    let staged = context
        .replace(
            &context
                .replace(&expression, &bind(&[(&m, 5)]))
                .expect("replacement"),
            &bind(&[(&n, 3)]),
        )
        .expect("replacement");
    assert_eq!(staged, both);
    // A name entry is the source-lookup adapter and binds by spelling.
    let mut named = ParamBindings::new();
    named.bind_name("n", literal(&context, 3));
    named.bind_name("m", literal(&context, 5));
    assert_eq!(context.replace(&expression, &named).expect("named"), both);
}

#[test]
fn literal_and_machine_arguments_agree_after_contextual_conversion() {
    let context = ParamContext::new();
    let from_literal = context
        .constant_as(CtValue::IntLiteral(IntLiteral::from(3_i64)), &Ty::Int)
        .expect("a literal converts to Int");
    let from_machine = context.constant(CtValue::Int(3)).expect("a constant");
    assert_eq!(from_literal, from_machine);
    // Exact literal arithmetic is not narrowed.
    let big = context
        .constant(CtValue::IntLiteral(IntLiteral::from(i64::MAX)))
        .expect("a constant");
    let exact = infix(&context, InfixOp::Add, &big, &literal(&context, 1));
    assert_eq!(
        exact.as_constant(),
        Some(&CtValue::IntLiteral(
            IntLiteral::from(i64::MAX).add(&IntLiteral::from(1_i64))
        ))
    );
    // Machine arithmetic wraps, and a literal beside it converts first.
    let machine_max = context
        .constant(CtValue::Int(i64::MAX))
        .expect("a constant");
    let wrapped = infix(&context, InfixOp::Add, &machine_max, &literal(&context, 1));
    assert_eq!(wrapped.as_constant(), Some(&CtValue::Int(i64::MIN)));
}

#[test]
fn signature_slots_are_alpha_equivalent_and_capture_avoiding() {
    let context = ParamContext::new();
    let slot = context.index_ref(0, 0, MetaTy::int());
    let outer = context.index_ref(1, 0, MetaTy::int());
    let body = infix(&context, InfixOp::Add, &slot, &outer);

    // Binding the outer frame from depth 1 leaves the inner slot alone and
    // shifts the replacement's own free slots past the binder it crossed.
    let mut bindings = ParamBindings::new();
    bindings.push_frame(vec![Some(context.index_ref(0, 7, MetaTy::int()))]);
    let replaced = context
        .replace_at(&body, &bindings, 1, &mut HashMap::new())
        .expect("replacement");
    let expected = infix(
        &context,
        InfixOp::Add,
        &slot,
        &context.index_ref(1, 7, MetaTy::int()),
    );
    assert_eq!(replaced, expected);
}

#[test]
fn constants_keep_bits_order_and_field_identity() {
    let context = ParamContext::new();
    let zero = context
        .constant(CtValue::Float(0.0_f64.to_bits()))
        .expect("constant");
    let negative_zero = context
        .constant(CtValue::Float((-0.0_f64).to_bits()))
        .expect("constant");
    assert_ne!(zero, negative_zero, "float identity is by bits");
    let tuple = |values: Vec<CtValue>| context.constant(CtValue::Tuple(values)).expect("constant");
    assert_ne!(
        tuple(vec![CtValue::Int(1), CtValue::Int(2)]),
        tuple(vec![CtValue::Int(2), CtValue::Int(1)])
    );
    let point = |x, y| CtValue::Struct {
        name: "Point".to_string(),
        fields: vec![
            ("x".to_string(), CtValue::Int(x)),
            ("y".to_string(), CtValue::Int(y)),
        ],
    };
    assert_ne!(
        context.constant(point(1, 2)).expect("constant"),
        context.constant(point(2, 1)).expect("constant")
    );
    assert_eq!(
        context.constant(point(1, 2)).expect("constant").meta(),
        &MetaTy::value(Ty::Struct("Point".to_string(), Vec::new()))
    );
    // A display's spelling is materialization metadata, not identity.
    let bare = CtValue::set(None, vec![CtValue::Int(1)]);
    let spelled = CtValue::set(Some(crate::types::set_type(Ty::Int)), vec![CtValue::Int(1)]);
    assert!(identity_eq(&bare, &spelled));
    assert_eq!(
        hash_of(&TyArg::Val(bare.clone())),
        hash_of(&TyArg::Val(spelled.clone()))
    );
    assert_eq!(TyArg::Val(bare), TyArg::Val(spelled));
}

#[test]
fn equal_types_hash_equally_and_interning_keeps_decorations() {
    let plain = callable(TransferSet::default());
    let decorated = callable(one_transfer());
    assert_eq!(plain, decorated, "transfer effects are not type identity");
    assert_eq!(hash_of(&plain), hash_of(&decorated));

    let context = ParamContext::new();
    let first = context.type_shape(plain);
    let second = context.type_shape(decorated);
    assert_eq!(first, second);
    let effects = |expr: &ParamExpr| match expr.as_constant() {
        Some(CtValue::Type(ty)) => match &**ty {
            Ty::Func { transfers, .. } => transfers.0.len(),
            _ => unreachable!("a callable type"),
        },
        _ => unreachable!("a closed type constant"),
    };
    assert_eq!(effects(&first), 0);
    assert_eq!(
        effects(&second),
        1,
        "the decorated occurrence keeps its own effects"
    );
}

#[test]
fn invalid_nodes_are_rejected() {
    let context = ParamContext::new();
    let n = int_param(&context, "f", 0, "n");
    let flag = context.register(ParamId::new("f", 1), "flag", MetaTy::bool());
    assert!(matches!(
        context.infix(InfixOp::Add, &n, &flag),
        Err(ParamError::TypeMismatch { .. })
    ));
    assert!(matches!(
        context.op(ParamOp::Cond, &[flag.clone(), n.clone()]),
        Err(ParamError::Arity { .. })
    ));
    assert!(matches!(
        context.op(ParamOp::BoolAnd, &[flag, n.clone()]),
        Err(ParamError::TypeMismatch { .. })
    ));
    assert!(matches!(
        context.conforms(&n, "Copyable"),
        Err(ParamError::TypeMismatch { .. })
    ));
    assert!(matches!(
        context.constant(CtValue::Tuple(vec![CtValue::Expr(n)])),
        Err(ParamError::NotConstant(_))
    ));
}

#[test]
fn partial_operators_stay_unevaluated_until_required() {
    let context = ParamContext::new();
    let one = context.constant(CtValue::Int(1)).expect("constant");
    let zero = context.constant(CtValue::Int(0)).expect("constant");
    let divided = infix(&context, InfixOp::FloorDiv, &one, &zero);
    assert!(divided.as_constant().is_none());
    assert!(divided.is_closed());
    assert_eq!(
        divided.require_constant(),
        Err(ParamError::Arithmetic("division by zero".to_string()))
    );
    // A cancellation does not erase the partial atom's error.
    let cancelled = infix(&context, InfixOp::Sub, &divided, &divided);
    assert!(cancelled.as_constant().is_none());
}

#[test]
fn propositions_are_three_valued() {
    let context = ParamContext::new();
    let n = int_param(&context, "f", 0, "n");
    let m = int_param(&context, "f", 1, "m");
    let increment = infix(&context, InfixOp::Add, &n, &literal(&context, 1));
    let equal = infix(&context, InfixOp::Eq, &increment, &m);
    assert!(matches!(
        ConstraintVerdict::from(equal.clone()),
        ConstraintVerdict::Residual(_)
    ));
    // Negating a residual is a residual, never a proof.
    let negated = context.not(&equal).expect("negation builds");
    assert!(matches!(
        ConstraintVerdict::from(negated.clone()),
        ConstraintVerdict::Residual(_)
    ));
    assert_eq!(context.not(&negated).expect("negation builds"), equal);
    // Canonical identity proves, a constant difference refutes.
    let reordered = infix(&context, InfixOp::Add, &literal(&context, 1), &n);
    assert!(
        ConstraintVerdict::from(infix(&context, InfixOp::Eq, &increment, &reordered)).is_proven()
    );
    let plus_two = infix(&context, InfixOp::Add, &n, &literal(&context, 2));
    assert!(
        ConstraintVerdict::from(infix(&context, InfixOp::Eq, &increment, &plus_two)).is_disproven()
    );

    let constraint = ParamConstraint {
        proposition: equal,
        message: Some("increment required".to_string()),
        location: None,
    };
    let bound = |n_value, m_value| {
        let mut bindings = ParamBindings::new();
        bindings.bind_name("n", literal(&context, n_value));
        bindings.bind_name("m", literal(&context, m_value));
        constraint.verdict(&context, &bindings).expect("verdict")
    };
    assert!(bound(3, 4).is_proven());
    assert!(bound(3, 5).is_disproven());
    // False dominates `and`; a residual beside True stays residual.
    let falsum = context.boolean(false);
    let conjunction = context
        .op(ParamOp::BoolAnd, &[constraint.proposition.clone(), falsum])
        .expect("conjunction");
    assert_eq!(conjunction.as_bool(), Some(false));
}

#[test]
fn expansion_is_budgeted() {
    let context = ParamContext::new();
    // (a0 + ... + a69) squared has 2_485 monomials; cubed exceeds the budget.
    let mut sum = int_param(&context, "f", 0, "a0");
    for slot in 1..70 {
        let next = int_param(&context, "f", slot, &format!("a{slot}"));
        sum = infix(&context, InfixOp::Add, &sum, &next);
    }
    let squared = infix(&context, InfixOp::Mul, &sum, &sum);
    assert!(matches!(
        context.infix(InfixOp::Mul, &squared, &sum),
        Err(ParamError::Budget {
            limit: MAX_MONOMIALS
        })
    ));
}

#[test]
fn finite_selection_folds_to_its_type() {
    let context = ParamContext::new();
    let index = int_param(&context, "f", 0, "i");
    let selection = context
        .select(vec![Ty::Int, Ty::Bool], &index)
        .expect("selection builds");
    let dependent = DependentType::resolve(selection.clone());
    assert!(matches!(dependent, Ty::Dependent(_)));
    let mut bindings = ParamBindings::new();
    bindings.bind_name("i", literal(&context, 1));
    let resolved =
        crate::types::replace_parameters(&context, &dependent, &bindings, 0).expect("replacement");
    assert_eq!(resolved, Ty::Bool);
    // Holes never prove each other equal.
    assert_ne!(
        context.hole(HoleKind::Unknown, MetaTy::int()),
        context.hole(HoleKind::Unknown, MetaTy::int())
    );
}

/// Type identity replaces; a required value evaluates. The pinned Mojo keeps
/// `n // 2` at `n = 8` unfolded (`Buf[8 // 2]` is not `Buf[4]`), while a
/// default over the same expression has the value `4`.
#[test]
fn replacement_keeps_opaque_atoms_unfolded_and_evaluation_folds_them() {
    let context = ParamContext::new();
    let n = int_param(&context, "f", 0, "n");
    let halved = infix(&context, InfixOp::FloorDiv, &n, &literal(&context, 2));
    let shifted = infix(&context, InfixOp::Add, &halved, &literal(&context, 1));
    let mut bindings = ParamBindings::new();
    bindings.bind_name("n", literal(&context, 8));

    let replaced = context.replace(&shifted, &bindings).expect("replacement");
    assert!(replaced.as_constant().is_none(), "{replaced}");
    assert_eq!(replaced.to_string(), "8 // 2 + 1");
    assert_eq!(
        context.evaluate(&shifted, &bindings),
        Ok(ParamEval::Constant(CtValue::Int(5)))
    );
    // A literal expression has no parameter to substitute and folds when it
    // is built, as `Buf[8 // 2]` spelled in source does.
    let eight = context.constant(CtValue::Int(8)).expect("constant");
    assert_eq!(
        infix(&context, InfixOp::FloorDiv, &eight, &literal(&context, 2)).as_constant(),
        Some(&CtValue::Int(4))
    );
    // The polynomial part does re-fold under replacement.
    let successor = infix(&context, InfixOp::Add, &n, &literal(&context, 1));
    assert_eq!(
        context
            .replace(&successor, &bindings)
            .expect("replacement")
            .as_constant(),
        Some(&CtValue::Int(9))
    );
}
