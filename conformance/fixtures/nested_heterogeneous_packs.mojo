def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
    def nested[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
        return len(Tuple(*args^))

    return nested(1, "two", True) + len(Tuple(*args^))


def main():
    print(count(9, False))
