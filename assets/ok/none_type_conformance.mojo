# The `None` type is upstream's `!kgen.none`, which conforms to no trait,
# while the `NoneType` struct declares `Defaultable`, `ImplicitlyCopyable`,
# `TrivialRegisterPassable`, and `Writable`, as at the pin.


def main():
    comptime N = type_of(None)
    print(
        conforms_to(N, Writable),
        conforms_to(N, Defaultable),
        conforms_to(N, Copyable),
        conforms_to(N, TrivialRegisterPassable),
        conforms_to(N, AnyType),
    )
    comptime M = NoneType
    print(
        conforms_to(M, Writable),
        conforms_to(M, Defaultable),
        conforms_to(M, Copyable),
        conforms_to(M, TrivialRegisterPassable),
        conforms_to(M, Equatable),
    )
    var o = Optional[NoneType](None)
    print(o.__bool__())
