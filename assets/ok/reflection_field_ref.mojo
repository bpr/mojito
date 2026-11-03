# `reflect[T].field_ref[idx](s)` is a reference to field `idx` of `s`, with
# the argument's origin: it reads the field, a write through it stores into
# `s`, a `ref` binding of it aliases the field, and a temporary argument
# lives for the statement.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int


@fieldwise_init
struct Named(Copyable):
    var n: Int
    var s: String


def main():
    var p = P(3, 4)
    print(reflect[P].field_ref[0](p), reflect[P].field_ref[1](p))
    reflect[P].field_ref[0](p) = 42
    print(p.x)
    reflect[P].field_ref[1](p) += 1
    print(p.y)
    comptime r = reflect[P]
    print(r.field_ref[1](p))
    print(reflect[P].field_ref[0](P(5, 6)))
    var q = Named(3, "hi")
    ref s = reflect[Named].field_ref[1](q)
    s += "!"
    print(q.s)
    var t = reflect[Named].field_ref[1](q).copy()
    print(t)
