# expect: lacking evidence to prove correctness
# A `where` clause over an opaque atom finds no evidence even at a concrete
# application: the pin does not fold `n // -2` at `n = 7` to prove `== -4`.
def floored[n: Int]() -> Int where n // -2 == -4:
    return n


def main():
    print(floored[7]())
