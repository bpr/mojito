# A bundled generic `def` called with a loan-carrying type argument
# (`alloc(Layout[Span[Int, origin_of(xs)]](...))`, `dealloc`) is cloned as a
# user function's call is: each origin slot of the argument becomes an
# origin binder the clone declares, inferred from the call's own arguments,
# and the result binds it too, so the allocation holds values of the
# argument's own type.
from std.memory import Layout, dealloc


def main():
    var xs: List[Int] = [1, 2, 3]
    var a = alloc(Layout[Span[Int, origin_of(xs)]](count=2))
    var p = a.unsafe_ptr()
    p.unsafe_offset(0).unsafe_write(Span(xs))
    p.unsafe_offset(1).unsafe_write(Span(xs))
    print(len(p[0]), p[1][2], a.layout().count())
    dealloc(a^)
    print(xs[0])
