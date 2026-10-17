# A nested generic `def` is a generator over its own parameters beside its
# enclosing scope's, so it keys its instances only on those it reads, as
# upstream's `RemoveUnusedParams` treats the lifted function: `inner[1]`
# and `inner[2]` share one instance when `inner` never reads `m`, and an
# enclosing function that only forwards its parameter to that removed `m`
# keeps one instance too, while a body reading `m`, directly, in its
# signature, through its own nested body, or by passing it to a callee
# that reads it, keeps one per value.
def leaf[k: Int](x: Int) -> Int:
    return x - 1


def keeper[k: Int](x: Int) -> Int:
    return x + k


def ignores[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return y + 1

    return inner[1](x) + inner[2](x)


def reads[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return y + m

    return inner[1](x) + inner[2](x)


def forwards[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return y * 2

    return inner[n](x)


def reads_outer[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return y + n

    return inner[1](x) + inner[2](x)


def through_leaf[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return leaf[m](y)

    return inner[1](x) + inner[2](x)


def through_keeper[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        return keeper[m](y)

    return inner[1](x) + inner[2](x)


def deeper[n: Int](x: Int) -> Int:
    def inner[m: Int](y: Int) -> Int:
        def leafy(z: Int) -> Int:
            return z + m

        return leafy(y)

    return inner[1](x) + inner[2](x)


def typed[n: Int](x: Int) -> Int:
    def inner[T: Copyable & Writable](v: T) -> Int:
        return 7

    return inner(x) + inner(String("a"))


def defaulted[n: Int](x: Int) -> Int:
    def inner[m: Int = 3](y: Int) -> Int:
        return y + 4

    return inner(x) + inner[5](x)


def keyword[n: Int](x: Int) -> Int:
    def inner[a: Int, b: Int](y: Int) -> Int:
        return y + b

    return inner[a=1, b=2](x) + inner[a=3, b=2](x)


def main():
    print(ignores[1](3), ignores[2](4))
    print(reads[1](3), reads[2](4))
    print(forwards[1](3), forwards[2](4))
    print(reads_outer[1](3), reads_outer[2](4))
    print(through_leaf[1](3), through_leaf[2](4))
    print(through_keeper[1](3), through_keeper[2](4))
    print(deeper[1](3), deeper[2](4))
    print(typed[1](3), typed[2](4))
    print(defaulted[1](3), defaulted[2](4))
    print(keyword[1](3), keyword[2](4))
