# PROBE (re-probe): a template reading a reflection handle is validated from
# its template and still keeps the clone check.
#
# Both compilers print 2, 0. Source validation checks the body once with `T`
# symbolic (`reflect[T].field_count()` is a compile-time `Int` there), and
# its certificate is incomplete by rule, so every instance is checked as a
# clone with the field facts the elaborator evaluates. Re-run whenever
# discovery scheduling or the certificate rules change.
#
# A narrower question stays open: the pin answers `reflect[Int].field_count()`
# with 0, while Mojito rejects a non-struct operand ("requires a struct type").
# Roadmap section 3 carries it.
#
# Observed 2026-09-23 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
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
