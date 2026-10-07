# expect: type mismatch for value parameter 'p'
# A tuple argument element converts to a `String` element only from a string.
def g[p: Tuple[Int, String]]():
    print(p[0], p[1])


def main():
    g[(1, 2.5)]()
