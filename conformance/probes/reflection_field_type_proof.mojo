# PROBE (re-probe): a `conforms_to` arm licenses exactly its trait on a
# symbolic field type.
#
# `reflect[T].field_types()[i]` is opaque: `T`'s own bound says nothing about
# a field (the pin: "does not conform to trait 'Writable'; either prove the
# conformance with 'conforms_to', or add conformance"). Inside
# `comptime if conforms_to(types[i], Defaultable & Writable):` the arm may
# construct and print the field type; `Sized` was not proved, so `len(...)`
# on the same value is rejected by both compilers. Delete the `len` line to
# see the program accepted uninstantiated. The enforced claims are
# `assets/ok/reflection_symbolic_fields.mojo` and
# `assets/type_error/reflected_field_type_needs_proof.mojo`.
#
# Observed 2026-09-23 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run reflection_field_type_proof.mojo
#         cargo run -- run conformance/probes/reflection_field_type_proof.mojo
def show[T: AnyType]() -> Int:
    comptime r = reflect[T]
    comptime types = r.field_types()
    var n = 0
    comptime for i in range(r.field_count()):
        comptime if conforms_to(types[i], Defaultable & Writable):
            print(types[i]())
            n += len(types[i]())
    return n


def main():
    print("never instantiated")
