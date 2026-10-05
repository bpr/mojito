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
# A narrower question stays open: the pin answers `reflect[Int].field_count()`
# through a generic `def` with 1, while Mojito rejects a non-struct operand
# ("requires a struct type"). Roadmap R76 carries it.
#
# Observed 2026-09-23 and 2026-10-05 against
# `Mojo 1.2.0.dev2026092105 (e9569894)`.
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
