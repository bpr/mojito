# expect: cannot transfer out of the read pack 'a'
# A read pack (`*a`) is borrowed from the caller, so `*a^` has nothing to
# move.
def sink[*Ts: Copyable](*b: *Ts) -> Int:
    return len(b)


def relay[*Ts: Copyable](*a: *Ts) -> Int:
    return sink(*a^)


def main():
    print(relay(1, True))
