use mojito::mir::{MirInstr, lower_checked_program};
use mojito::{BackendKind, Compiler, CompilerError, LinkOptions, OwnershipError, elaborate, parse};

const PARAMETRIC_CAPTURE: &str = r#"
def invoke[
    origins: OriginSet, //, callback: def(Int) capturing[origins] -> Int
](value: Int) -> Int:
    return callback(value)

def main():
    var base = 40

    @parameter
    def add(value: Int) -> Int:
        return base + value

    print(invoke[add](2))
"#;

const GENERIC_ANONYMOUS_CALLABLE: &str = r#"
def identity[U: ImplicitlyCopyable & ImplicitlyDeletable](value: U) -> U:
    return value

def invoke[
    callback: def[T: ImplicitlyCopyable & ImplicitlyDeletable](T) thin -> T
](value: Int) -> Int:
    return callback(value)

def main():
    print(invoke[identity](42))
"#;

const CALLABLE_PARAMETER_DEFAULTS: &str =
    include_str!("../conformance/fixtures/callable_parameter_defaults.mojo");
const GENERIC_ANONYMOUS_CONFORMANCE: &str =
    include_str!("../conformance/fixtures/generic_anonymous_callables.mojo");

#[test]
fn callable_value_parameter_is_a_hidden_typed_mir_local() {
    let parsed = parse(PARAMETRIC_CAPTURE).expect("parse callable value parameter");
    let elaborated = elaborate(parsed).expect("elaborate callable value parameter");
    let checked = mojito::check_program(&elaborated).expect("check callable value parameter");
    let mir = lower_checked_program(&checked);
    assert!(
        mir.invariant_errors.is_empty(),
        "{:?}",
        mir.invariant_errors
    );

    let invoke = mir
        .functions
        .iter()
        .find(|(name, _)| name == "invoke")
        .map(|(_, function)| function)
        .expect("invoke MIR function");
    assert_eq!(
        invoke.n_params, 1,
        "only `value` is an ordinary ABI parameter"
    );
    assert!(invoke.var_names.iter().any(|name| name == "callback"));
    assert!(invoke.blocks.iter().any(|block| {
        block
            .instrs
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::CallIndirect { .. }))
    }));
}

#[test]
fn callable_value_parameter_executes_with_its_reified_closure() {
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(PARAMETRIC_CAPTURE)
        .expect("compile callable value parameter");
    let execution = compiler
        .execute(&compiled)
        .expect("execute callable value parameter");
    assert_eq!(execution.output, "42\n");
}

#[test]
fn generic_anonymous_callable_value_executes_with_inferred_inner_parameters() {
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(GENERIC_ANONYMOUS_CALLABLE)
        .expect("compile generic anonymous callable value");
    let execution = compiler
        .execute(&compiled)
        .expect("execute generic anonymous callable value");
    assert_eq!(execution.output, "42\n");
}

#[test]
fn callable_parameter_defaults_reify_symbols_aliases_and_conditionals() {
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(CALLABLE_PARAMETER_DEFAULTS)
        .expect("compile callable defaults");
    let execution = compiler
        .execute(&compiled)
        .expect("execute callable defaults");
    assert_eq!(execution.output, "42\n42\n42\n41\n42\n");
}

#[test]
fn generic_callable_contract_defaults_override_implementation_defaults() {
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(GENERIC_ANONYMOUS_CONFORMANCE)
        .expect("compile generic callable conformance");
    let execution = compiler
        .execute(&compiled)
        .expect("execute generic callable conformance");
    assert_eq!(execution.output, "42\n42\n42\n42\n43\n102\n42\n42\n42\n");

    let parsed = parse(GENERIC_ANONYMOUS_CONFORMANCE).expect("parse generic callable conformance");
    let elaborated = elaborate(parsed).expect("elaborate generic callable conformance");
    let checked = mojito::check_program(&elaborated).expect("check generic callable conformance");
    let mir = lower_checked_program(&checked);
    let function = mir
        .functions
        .iter()
        .find(|(name, _)| name == "invoke_contract_default")
        .map(|(_, function)| function)
        .expect("contract-default function");
    assert!(function.blocks.iter().any(|block| {
        block.instrs.iter().any(|instruction| {
            matches!(instruction, MirInstr::CallIndirect { param_arg_regs, param_decls, .. }
            if param_arg_regs.is_empty() && !param_decls.is_empty())
        })
    }));
}

#[test]
fn named_callable_argument_skips_an_earlier_default_without_shifting() {
    let source = r#"
def increment(value: Int) -> Int:
    return value + 1

def decrement(value: Int) -> Int:
    return value - 1

def apply[
    enabled: Bool = True,
    callback: def(Int) thin -> Int = increment,
](value: Int) -> Int:
    return callback(value)

def main():
    print(apply[callback=decrement](42))
"#;
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(source)
        .expect("compile named callable argument");
    let execution = compiler
        .execute(&compiled)
        .expect("execute named callable argument");
    assert_eq!(execution.output, "41\n");
}

#[test]
fn partial_generic_callable_invocation_uses_contract_scalar_defaults() {
    let source = r#"
def positional_actual[n: Int = 0, m: Int = 3](value: Int) -> Int:
    return value + n + m

def positional[
    function: def[n: Int = 0, m: Int = 2](Int) thin -> Int
]() -> Int:
    return function[1](40)

def named_actual[scale: Int = 7, n: Int = 1](value: Int) -> Int:
    return value + scale + n

def named[
    function: def[scale: Int = 100, n: Int = 1](Int) thin -> Int
]() -> Int:
    return function[n=2](0)

def main():
    print(positional[positional_actual]())
    print(named[named_actual]())
"#;
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(source)
        .expect("compile partial callable invocation");
    let execution = compiler
        .execute(&compiled)
        .expect("execute partial callable invocation");
    assert_eq!(execution.output, "43\n102\n");
}

#[test]
fn generic_callable_contract_resolves_callable_default_plans() {
    let source = r#"
def increment(value: Int) -> Int:
    return value + 1

def decrement(value: Int) -> Int:
    return value - 1

def symbol_actual[
    callback: def(Int) thin -> Int = decrement
](value: Int) -> Int:
    return callback(value)

def symbol_contract[
    function: def[callback: def(Int) thin -> Int = increment](Int) thin -> Int
]() -> Int:
    return function(41)

def conditional_actual[
    enabled: Bool = False,
    callback: def(Int) thin -> Int = increment if enabled else decrement,
](value: Int) -> Int:
    return callback(value)

def conditional_contract[
    function: def[
        enabled: Bool = True,
        callback: def(Int) thin -> Int = increment if enabled else decrement,
    ](Int) thin -> Int
]() -> Int:
    return function(41)

def alias_actual[
    primary: def(Int) thin -> Int = decrement,
    fallback: def(Int) thin -> Int = primary,
](value: Int) -> Int:
    return fallback(value)

def alias_contract[
    function: def[
        primary: def(Int) thin -> Int = increment,
        fallback: def(Int) thin -> Int = primary,
    ](Int) thin -> Int
]() -> Int:
    return function(41)

def main():
    print(symbol_contract[symbol_actual]())
    print(conditional_contract[conditional_actual]())
    print(alias_contract[alias_actual]())
"#;
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(source)
        .expect("compile callable contract default plans");
    let execution = compiler
        .execute(&compiled)
        .expect("execute callable contract default plans");
    assert_eq!(execution.output, "42\n42\n42\n");
}

#[test]
fn callable_value_capture_effect_conflicts_with_a_live_owner_loan() {
    let source = r#"
def invoke[
    origins: OriginSet, //, callback: def() capturing[origins]
]():
    callback()

def main():
    var value = 1
    ref alias = value

    @parameter
    def replace() {mut value}:
        value = 2

    invoke[replace]()
    print(alias)
"#;
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    assert!(matches!(
        compiler.compile_unlinked(source),
        Err(CompilerError::Ownership(OwnershipError::LoanConflict { place, loan, .. }))
            if place == "value" && loan == "alias"
    ));
}

#[test]
fn generic_callable_expression_capture_effect_conflicts_with_a_live_owner_loan() {
    let source = r#"
def invoke[
    origins: OriginSet, //, callback: def() capturing[origins] -> None
]():
    callback()

def main():
    var value = 1

    @parameter
    def replace() {mut value}:
        value = 2

    ref alias = value
    var functions = (invoke,)
    functions[0][replace]()
    print(alias)
"#;
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let result = compiler.compile_unlinked(source);
    assert!(
        matches!(
            &result,
            Err(CompilerError::Ownership(OwnershipError::LoanConflict { place, loan, .. }))
                if place == "value" && loan == "alias"
        ),
        "got {result:?}"
    );
}

#[test]
fn generic_callable_tuple_preserves_its_checked_parameter_contract() {
    let source = r#"
def identity[T: ImplicitlyCopyable & ImplicitlyDeletable](value: T) -> T:
    return value

def offset[n: Int = 1](value: Int) -> Int:
    return value + n

def main():
    var functions = (identity, offset)
    print(functions[0][Int](42))
    print(functions[1][2](40))
"#;
    let compiler = Compiler::new(LinkOptions::default(), BackendKind::Vm);
    let compiled = compiler
        .compile_unlinked(source)
        .expect("compile generic callable Tuple elements");
    let execution = compiler
        .execute(&compiled)
        .expect("execute generic callable Tuple elements");
    assert_eq!(execution.output, "42\n42\n");
}
