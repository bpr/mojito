# expect: does not conform to trait 'Defaultable'
# A pack element constructs only when the pack's bound or a `where` clause
# makes it `Defaultable`.
def build[*Ts: Movable & Writable & Deinitable]():
    comptime for i in range(len(Ts)):
        var value = Ts[i]()
        print(value)


def main():
    build[Int]()
