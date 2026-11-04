# expect: unqualified access to struct parameter 'T'; use 'Self.T' instead
# A method's `where` clause reads its struct's parameter as `Self.T`; the
# bare spelling is upstream's name-lookup error, as in the method's body.
from std.traits import IsTriviallyCopyable


struct W[T: AnyType]:
    def __init__(out self):
        pass

    def f(self) -> Int where IsTriviallyCopyable[T]:
        return 1


def main():
    var w = W[Int]()
    print(w.f())
