# A conformance-list entry may be a trait composition: `A & B` conforms to
# each trait, `A & B where C` conditions each, and `not (A & B)` (or
# `not A & B`, where `not` takes the whole composition) opts out of each.
# Trait refinement lists take compositions as well.


trait Base:
    pass


trait Other:
    pass


trait Both(Base & Other):
    pass


def need_base[T: Base](x: T):
    print("base")


def need_other[T: Other](x: T):
    print("other")


@fieldwise_init
struct Plain(Base & Other):
    var x: Int


@fieldwise_init
struct Nested((Base & Other), Copyable):
    var x: Int


@fieldwise_init
struct Conditional[T: Movable](Base & Other where conforms_to(T, Copyable)):
    var x: Int


@fieldwise_init
struct OptedOut(not (Base & Other), Movable):
    var x: Int


@fieldwise_init
struct OptedOutWithReason(not Base & Other else "not a plain payload", Movable):
    var x: Int


@fieldwise_init
struct Refined(Both):
    var x: Int


def main():
    print(Plain(1).x)
    need_base(Plain(2))
    need_other(Plain(3))
    need_other(Nested(4))
    need_base(Conditional[Int](5))
    print(OptedOut(6).x, OptedOutWithReason(7).x)
    print(conforms_to(OptedOut, Base), conforms_to(OptedOutWithReason, Other))
    need_other(Refined(8))
