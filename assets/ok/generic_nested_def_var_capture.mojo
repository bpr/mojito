# A generic nested `def` with a by-value capture (`{var x}`) snapshots `x`
# when it is declared, so a later `x = 100` does not reach it: each
# instance's call reads the snapshot the frame holds, and the pin prints 10.
def outer() -> Int:
    var x = 5

    def inner[k: Int]() {var x} -> Int:
        return x * k

    x = 100
    return inner[2]()


def main():
    print(outer())
