# PROBE (re-probe): a conformance proof on a symbolic field type is keyed by
# the exact element and licenses only what follows it in its block.
#
# The pin rejects all three bodies uninstantiated, each construction with
# "invalid call to '__init__': no candidates found": a proof on `types[0]`
# does not license `types[i]`; a proof after the use licenses nothing before
# it; a proof in one loop does not reach another. Mojito rejects likewise,
# naming the missing `Defaultable` bound. The enforced claim is
# `assets/type_error/reflected_field_proof_is_positional.mojo`.
#
# Observed 2026-09-23 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run reflection_proof_is_positional.mojo
#         cargo run -- run conformance/probes/reflection_proof_is_positional.mojo
def other_index[T: AnyType]():
    comptime r = reflect[T]
    comptime types = r.field_types()
    comptime for i in range(r.field_count()):
        comptime if conforms_to(types[0], Defaultable & Writable):
            print(types[i]())


def after_use[T: AnyType]():
    comptime r = reflect[T]
    comptime types = r.field_types()
    comptime for i in range(r.field_count()):
        print(types[i]())
        comptime if conforms_to(types[i], Defaultable & Writable):
            pass


def other_loop[T: AnyType]():
    comptime r = reflect[T]
    comptime types = r.field_types()
    comptime for i in range(r.field_count()):
        comptime if conforms_to(types[i], Defaultable & Writable):
            pass
    comptime for j in range(r.field_count()):
        print(types[j]())


def main():
    print("never instantiated")
