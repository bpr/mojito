# A collector indexed by a runtime value reads the element at that index,
# both as a value (`xs[i]`) and as the receiver of a method call; an
# instance with an empty collector compiles, its guarded reads never run.


def total(*xs: Int) -> Int:
    var sum = 0
    for i in range(len(xs)):
        sum += xs[i]
    return sum


def pick(i: Int, *xs: Float64) -> Float64:
    return xs[i]


def count(*xs: String) -> Int:
    var n = 0
    for i in range(len(xs)):
        n += xs[i].byte_length()
    return n


def first_len(*xs: String) -> Int:
    if len(xs) > 0:
        return xs[0].byte_length()
    return -1


def show(var *xs: String):
    for i in range(len(xs)):
        print(xs[i])


def main():
    print(total())
    print(total(4))
    print(total(1, 2, 3))
    print(pick(1, 1.5, 2.5))
    print(count())
    print(count("ab", "cde"))
    print(first_len())
    print(first_len("xyz"))
    show("p", "q")
