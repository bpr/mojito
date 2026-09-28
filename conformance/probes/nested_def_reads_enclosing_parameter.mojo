# A generic nested `def` reading its enclosing function's value parameter
# (`n` inside `inner[k]`) runs on the VM and prints 32 and 64, as the pin
# does. Natively it is refused: "generic retained callable `outer$inner` has
# captures" — Mojito reads `n` through a capture of its runtime slot, and
# native monomorphization cannot yet specialize a generic nested `def` that
# carries an environment.
def scaled[n: Int]() -> Int:
    return n


def outer[n: Int]() -> Int:
    def inner[k: Int]() -> Int:
        return scaled[k + 1]() * n

    return inner[10]() + inner[20]()


def main():
    print(outer[1]())
    print(outer[2]())
