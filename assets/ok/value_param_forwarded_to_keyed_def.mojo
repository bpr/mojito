# A value-keyed `def` forwards its own value parameter to a `def` whose body
# branches on it at compile time (`keyed[n]()`): each instance of `forward`
# calls the instance of `keyed` at its own `n`. The pinned Mojo prints `4 1`.
def keyed[n: Int]() -> Int:
    comptime if n > 2:
        return n
    return 0


def forward[n: Int](x: Int) -> Int:
    return keyed[n]() + x


def main():
    print(forward[3](1), forward[1](1))
