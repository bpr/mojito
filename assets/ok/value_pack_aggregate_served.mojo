def g[*vs: Tuple[Int, Int]]() -> Int:
    return 1


def main():
    print(g[(1, 2), (3, 4)]())
