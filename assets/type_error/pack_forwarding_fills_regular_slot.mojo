# expect: unpack is only supported when the callee accepts a variadic pack
# A forwarded pack is the positional collector's one argument; it cannot
# fill a regular parameter on its way there.
def sink[*Ts: Copyable](head: Int, *b: *Ts) -> Int:
    return head


def relay[*Ts: Copyable](*a: *Ts) -> Int:
    return sink(*a)


def main():
    print(relay(1, 2))
