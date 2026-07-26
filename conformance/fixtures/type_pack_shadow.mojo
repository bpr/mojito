def inspect[*Ts: Copyable & ImplicitlyDeletable](*args: *Ts):
    def nested[Ts: AnyType](value: Tuple[*Ts]) -> Int:
        return len(value)

    print(nested[Int](Tuple(1, True)))


def main():
    inspect(9, False)
