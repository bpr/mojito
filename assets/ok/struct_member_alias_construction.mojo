# A construction through a struct's parameterized type member constructs
# the struct the member names, as at the pin: `S[Int].Of[Bool](3, 4)` is
# a `Tuple[Int, Int]` and `B.Of[Int]()` an empty `List[Int]`.
struct S[T: AnyType]:
    comptime Of[U: AnyType] = Tuple[Int, Int]


struct B:
    comptime Of[T: Copyable] = List[T]


def main():
    var t = S[Int].Of[Bool](3, 4)
    print(t[0], t[1])
    var xs = B.Of[Int]()
    xs.append(9)
    print(len(xs), xs[0])
