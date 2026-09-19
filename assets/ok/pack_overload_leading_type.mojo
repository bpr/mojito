# Two type-pack overloads told apart by a regular parameter's type. Both
# calls bind the same pack, so both clones share a specialization name and
# stand as ordinary overloads of it.
# requires: discovery


def collect[*Ts: Writable](first: Int, *rest: *Ts) -> Int:
    return 1 + len(rest)


def collect[*Ts: Writable](first: String, *rest: *Ts) -> Int:
    return 100 + len(rest)


def main():
    print(collect(1, "a", 2.0))
    print(collect(1, "a"))
    print(collect(String("x"), "a"))
