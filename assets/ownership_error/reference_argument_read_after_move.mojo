# expect: use of uninitialized value 'tup'
# A by-value tuple element argument is read at call setup, after a later
# argument moved the tuple away: the pin rejects the read of the moved value.
def take(var t: Tuple[Int, Int]) -> Int:
    return t[1]


def two(a: Int, b: Int):
    print("two", a, b)


def main():
    var tup = (3, 4)
    two(tup[0], take(tup^))
