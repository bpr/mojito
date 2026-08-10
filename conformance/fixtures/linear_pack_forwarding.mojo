def collect[*Ts: Movable & Deinitable](
    head: Int, var *items: *Ts, tail: Int
) -> Int:
    return head + len(items) + tail


def relay[*Ts: Movable & Deinitable](var *items: *Ts) -> Int:
    return collect(30, *items^, tail=10)


def main():
    print(relay([1], True))
