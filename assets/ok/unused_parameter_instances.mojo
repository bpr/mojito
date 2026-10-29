# A generic function whose body and signature never read one of its
# compile-time parameters has one instance for every value of it, as
# upstream's `RemoveUnusedParams` leaves it: a read only the cleanup
# passes erase (`if False`) or only another parameter's default makes
# keeps no parameter alive, while a nested function's read still does.
@no_inline
def constant[n: Int]() -> Int:
    return 1

@no_inline
def add_k[T: AnyType, k: Int](x: Int) -> Int:
    return x + k

struct Box[n: Int]:
    var v: Int

    @no_inline
    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def get[m: Int](self) -> Int:
        return self.v + Self.n

@no_inline
def nested_read[n: Int]() -> Int:
    @no_inline
    def inner() -> Int:
        return n

    return inner()

@no_inline
def dead_read[n: Int]() -> Int:
    if False:
        return n
    return 0

@no_inline
def default_only[a: Int, b: Int = a]() -> Int:
    return b

def main():
    print(constant[1]() + constant[2]() + constant[3]())
    print(add_k[Int, 1](1) + add_k[String, 1](1))
    var b = Box[3](4)
    print(b.get[1]() + b.get[2]())
    print(nested_read[1](), nested_read[2]())
    print(dead_read[1]() + dead_read[2]())
    print(default_only[1, 2]() + default_only[3, 2]())
