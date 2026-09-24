# PROBE (re-probe): a body reading a reflection handle is checked before
# anything instantiates it.
#
# Both compilers reject `f` from the template, with nothing calling it: the
# pin with "cannot implicitly convert 'StringLiteral["oops"]' value to
# 'Int'", Mojito with its type mismatch at the same line. The enforced claims
# are `assets/type_error/untaken_comptime_if_reflection_def.mojo`,
# `untaken_comptime_for_reflection_body.mojo`, and
# `untaken_comptime_if_reflection_method.mojo`.
#
# Observed 2026-09-23 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run reflection_body_untaken_arm.mojo
#         cargo run -- run conformance/probes/reflection_body_untaken_arm.mojo
def f[T: AnyType]() -> Int:
    comptime if reflect[T].field_count() == 2:
        return 2
    else:
        var x: Int = "oops"
        return 0


def main():
    print("never instantiated")
