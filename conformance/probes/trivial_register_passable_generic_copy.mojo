# Probe: a generic `x.copy()` and `T(copy=x)` instantiated at a struct
# declaring `TrivialRegisterPassable` but not `Copyable`.
#
# The pin (2026-10-14) runs it and prints `1` twice. Mojito stops in the VM,
# "unsupported feature: vm: unknown method 'P.copy'".
# docs/roadmap.md R863. When Mojito runs it, promote this file to
# `assets/ok/` with its manifest rows.
@fieldwise_init
struct P(TrivialRegisterPassable, Writable):
    var a: Int


def by_method[T: Copyable](x: T) -> T:
    return x.copy()


def by_constructor[T: Copyable](x: T) -> T:
    return T(copy=x)


def main():
    print(by_method(P(1)).a)
    print(by_constructor(P(1)).a)
