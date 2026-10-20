# expect: does not match the signature required by trait
# A witness reading a different element of its own pack than the
# requirement reads does not satisfy it.
trait Getter:
    def get[*Us: AnyType](self, ref x: Us[1]) -> Int:
        ...


struct S(Getter):
    def __init__(out self):
        pass

    def get[*Vs: AnyType](self, ref x: Vs[0]) -> Int:
        return 9


def main():
    var s = S()
    var i = 3
    print(s.get[Int, Int](i))
