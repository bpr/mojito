def outer() -> Int:
    def score[*Ts: Movable & Deinitable](
        head: Int, var *args: *Ts
    ) -> Int:
        return head + len(args)

    def relay[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
        return score(40, *args^)

    return relay(1, True)


def main():
    print(outer())
