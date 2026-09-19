# Two type-pack overloads told apart by a keyword-only parameter after the
# collector.
# requires: discovery


def gather[*Ts: Writable](*rest: *Ts) -> Int:
    return len(rest)


def gather[*Ts: Writable](*rest: *Ts, sep: String) -> Int:
    return 100 + len(rest)


def main():
    print(gather(1, "a", 2.0))
    print(gather("x", "a", sep=","))
