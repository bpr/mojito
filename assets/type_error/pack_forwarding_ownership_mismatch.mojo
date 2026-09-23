# expect: cannot unpack a variadic pack into a call that requires a different ownership
# An owned pack (`var *a`) forwards into an owned collector only; a read
# collector takes a read pack.
def sink[*Ts: Copyable](*b: *Ts) -> Int:
    return len(b)


def relay[*Ts: Copyable & Movable](var *a: *Ts) -> Int:
    return sink(*a)


def main():
    print(relay(1, True))
