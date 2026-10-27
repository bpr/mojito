# A reflected field type (`.T` at the end of a handle chain) is an
# `AnyType` value: it binds a type parameter that asks nothing more of it,
# spelled in place or through a name, it names a type argument and an
# annotation through a local alias, and it is itself a reflection subject.
@fieldwise_init
struct P:
    var x: Int
    var y: Float64


@fieldwise_init
struct Q:
    var p: P
    var z: Int


def count_fields[T: AnyType]() -> Int:
    return reflect[T].field_count()


def main():
    print(count_fields[reflect[Q].field_at[0].T]())
    comptime r = reflect[Q]
    print(count_fields[r.field["p"].T]())
    print(reflect[reflect[Q].field_at[0].T].field_count())
    comptime s = reflect[P]
    var l = List[s.field_at[1].T]()
    l.append(1.5)
    print(len(l))
    comptime T = s.field["y"].T
    var a: T = 2.5
    print(a)
    print(reflect[reflect[P].field_at[0].T].is_struct())
