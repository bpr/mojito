# PROBE (open question): is a bare `def(...)` LOCAL `var` annotation a valid
# callable-value position in current Mojo, or does it name a trait there too?
#
# Context: the audited head treats a bare `def(...)` type position in struct
# fields and collection elements as a trait (Mojito now rejects those to
# match). Whether the same reading applies to a local binding annotation is
# unconfirmed. Mojito accepts this program and prints 42.
#
# Run:    mojo run def_typed_local_annotation.mojo        (audited pixi env)
#         cargo run -- run conformance/probes/def_typed_local_annotation.mojo
#
# If Mojo runs it: no action — the local channel is parity; delete the open
#   question from parity row functions.callable-values.
# If Mojo rejects it: local `def(...)` annotations are also a Mojito-only
#   extension — extend the slice-A4 gate to local annotations
#   (src/checker/type_resolution.rs reject_stored_callable_type) and update
#   parity rows functions.callable-values / features.md accordingly. The
#   generic_function_value_materialization probe is then moot.
def increment(x: Int) -> Int:
    return x + 1

def main():
    var callback: def(Int) -> Int = increment
    print(callback(41))
