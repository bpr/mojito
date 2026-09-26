# A value-keyed `def` with no compile-time control flow runs erased, its value
# passed at run time, so forwarding that value to a compile-time-keyed `def`
# (`keyed[n]()`) fails elaboration with "compile-time call arity: generic
# 'keyed' requires compile-time parameter 'n'". The pin prints "4 1".
def keyed[n: Int]() -> Int:
    comptime if n > 2:
        return n
    return 0


def forward[n: Int](x: Int) -> Int:
    return keyed[n]() + x


def main():
    print(forward[3](1), forward[1](1))
