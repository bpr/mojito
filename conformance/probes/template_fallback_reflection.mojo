# PROBE (re-probe): a template reading a reflection handle in a `comptime if`
# condition is served by its template.
#
# Both compilers print 2, 0. Source validation checks the body once with `T`
# symbolic (`reflect[T].field_count()` is a compile-time `Int` there). The
# condition reaches MIR as a `comptime if` thunk whose `reflect[T]` query is
# a parameter constant, which the elaborator answers per instance from the
# bound struct. Re-run whenever discovery scheduling, the certificate rules,
# or the elaborator's reflection answer change.
#
# A scalar bound to the same kind of template answers from `SIMD`'s one
# declared field (`assets/ok/reflection_simd_subject.mojo`).
#
# Observed 2026-09-23 and 2026-10-05 against
# `Mojo 1.2.0.dev2026092105 (e9569894)`, re-observed 2026-11-14 against
# `Mojo 1.3.0.dev2026110105`.
#
# Run:    mojo run template_fallback_reflection.mojo
#         cargo run -- run conformance/probes/template_fallback_reflection.mojo
@fieldwise_init
struct Pair(Copyable):
    var left: Int
    var right: Int


def field_count[T: AnyType]() -> Int:
    comptime if reflect[T].field_count() == 2:
        return 2
    else:
        return 0


@fieldwise_init
struct Single(Copyable):
    var only: Int


def main():
    print(field_count[Pair]())
    print(field_count[Single]())
