# A reflected field handle in an annotation (`s.field_at[1].T`,
# `s.field["x"].T`, `f.T` over a handle constant, and the inline
# `reflect[P].field_at[1].T`) is the field's type, built by the one
# compile-time builder like any other annotation.
@fieldwise_init
struct P(Copyable, Writable):
    var x: Int
    var y: Float64


def first_field[T: AnyType]() -> Int:
    comptime types = reflect[T].field_types()
    comptime n = len(types)
    return n


def main():
    comptime s = reflect[P]
    var a: s.field_at[1].T = 1.5
    print(a)
    var b: s.field["x"].T = 3
    print(b)
    comptime f = s.field_at[0]
    var c: f.T = 4
    print(c)
    print(first_field[P]())
    var d: reflect[P].field_at[1].T = 2.5
    print(d)
