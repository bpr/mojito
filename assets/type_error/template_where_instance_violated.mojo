# expect: n must stay below four
# A violated `where` clause on a compile-time-keyed template is a sourced
# constraint failure at the requesting instantiation, carrying the clause's
# own message. It is never a type error inside a clone's body.
def small[n: Int]() -> Int where (n < 4, "n must stay below four"):
    comptime if n == 0:
        return 100
    else:
        return 200


def tagged[T: Copyable](x: T) -> Int where conforms_to(T, Copyable):
    return 9


def main():
    print(small[0]())
    print(small[7]())
    print(tagged(1))
    print(tagged(True))
