# A `where` clause is the instance's obligation, not its body's: the requesting
# call and the elaborator discharge it before the instance exists, so a
# constrained template's instances still inherit its checked facts.
def small[n: Int]() -> Int where (n < 4, "n must stay below four"):
    comptime if n == 0:
        return 100
    else:
        return 200


def tagged[T: Copyable](x: T) -> Int where conforms_to(T, Copyable):
    return 9


def main():
    print(small[0]())
    print(small[3]())
    print(tagged(1))
    print(tagged(True))
