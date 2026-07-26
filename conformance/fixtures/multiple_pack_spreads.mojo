def collect[*Ts: Movable & ImplicitlyDeletable](var *items: *Ts) -> Int:
    return len(items)


def relay[*Ts: Movable & ImplicitlyDeletable](var *items: *Ts) -> Int:
    return collect(*items^, *items^)


def main():
    print(relay(1, True))
