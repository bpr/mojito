# `conforms_to(T, Tr)` is a compile-time `Bool` wherever it is read, as at
# the pin: a `comptime` binding and a run-time position alike, closed for a
# closed type and decided per instance over a binder.
def describe[T: AnyType]():
    comptime ordered = conforms_to(T, Comparable)
    print(ordered, conforms_to(T, Copyable & Hashable))


def main():
    print(conforms_to(Tuple[Int, Int], Comparable))
    print(conforms_to(Int, Copyable & Comparable))
    comptime boolable = conforms_to(String, Boolable)
    print(boolable)
    describe[Int]()
    describe[List[Int]]()
