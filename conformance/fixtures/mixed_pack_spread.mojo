def collect[*Ts: Movable & Deinitable](var *items: *Ts) -> Int:
    return len(items)


def relay[*Ts: Movable & Deinitable](var *items: *Ts) -> Int:
    return collect(*items^, 9)


def main():
    print(relay(1, True))
