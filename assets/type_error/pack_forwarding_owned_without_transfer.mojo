# expect: cannot be implicitly copied
# Forwarding an owned pack into an owned collector moves it, which needs the
# `^`: a pack is never implicitly copyable.
def sink[*Ts: Movable](var *b: *Ts) -> Int:
    return len(b)


def relay[*Ts: Movable](var *a: *Ts) -> Int:
    return sink(*a)


def main():
    print(relay(1, True))
