# expect: cannot unpack a variadic pack into a call that requires a different ownership
# A read pack spread into `Tuple`'s `var *args: *Self.Ts` collector is
# rejected, as at the pin: the construction selects the declared `__init__`,
# whose collector owns its elements.
def copied[*Ts: Copyable & Movable](*args: *Ts) -> Tuple[*Ts]:
    return Tuple[*Ts](*args)


def main():
    var t = copied(1, "x")
    print(t[0], t[1])
