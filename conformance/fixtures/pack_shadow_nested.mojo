def inspect[*Ts: Movable & ImplicitlyDeletable](var *args: *Ts):
    def nested(var args: Tuple[Int, Int]):
        print(Tuple(*args^))

    nested(Tuple(9, 10))


def main():
    inspect(1, True)
