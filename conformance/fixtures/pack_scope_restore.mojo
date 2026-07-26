def count[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts) -> Int:
    if True:
        var args = Tuple(7, 8)
        print(len(args))
    for args in [Tuple(9, 10)]:
        print(len(args))
    var packed = Tuple(*args^)
    return len(packed)


def main():
    print(count(1, "two", True))
