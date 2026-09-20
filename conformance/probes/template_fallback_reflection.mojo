# PROBE (re-probe): a template reading a reflection handle keeps the clone check.
#
# Both compilers print 2, 0. Only the elaborator evaluates `reflect[T]`, so
# source validation skips the body and it must never earn a template
# certificate. Re-run whenever discovery scheduling or fallback changes.
#
# A narrower question stays open: the pin answers `reflect[Int].field_count()`
# with 0, while Mojito rejects a non-struct operand ("requires a struct type").
# Roadmap section 3 carries it.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
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
