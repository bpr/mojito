# PROBE (open question): does current Mojo accept CONTEXTUAL materialization
# of an ordinary generic function into a function value (`identity`
# specialized to `def(Int) -> Int` by the annotation)?
#
# Context: the 2026-07-25 audit recorded that Mojito "materializes ordinary
# generic ... specializations as runtime values beyond forms accepted by the
# pinned nightly", but no differential case pinned this exact contextual
# form. Mojito accepts this program and prints 42. Only meaningful if the
# def_typed_local_annotation probe is accepted by Mojo at all.
#
# Run:    mojo run generic_function_value_materialization.mojo
#         cargo run -- run conformance/probes/generic_function_value_materialization.mojo
#
# If Mojo runs it: parity — remove the open question from parity row
#   functions.callable-values and add this as a differential `run` case.
# If Mojo rejects it: Mojito's contextual generic materialization is an
#   unintentional extension — gate it (the instantiation lives in
#   src/checker/generics.rs infer_specialized_callable_value /
#   contextual-value selection) and record the rejection with a fixture.
def identity[T: Copyable & Movable](value: T) -> T:
    return value

def main():
    var callback: def(Int) -> Int = identity
    print(callback(42))
