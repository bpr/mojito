# expect: has 'Copyable' type, but value has type 'AnyType'
# A reflected field type is an `AnyType` value, so it binds no type
# parameter whose bound asks more, through a local alias too, as at the pin.
@fieldwise_init
struct Q:
    var z: Int


def g[T: Copyable]() -> Int:
    return 1


def main():
    comptime F = reflect[Q].field_at[0].T
    print(g[F]())
