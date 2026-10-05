# expect: access to 'xs' conflicts with live reference 'm'
# A generic `def` its template serves stores a copy of its argument, which no
# argument expression hands over. Its transfer summary carries the stored
# type, and the call closes it with the type it binds to the `def`'s
# parameter: the list borrows `xs`, which the pointer type names, so `xs` is
# not written while the list is still read.
# requires: stdlib
def keep[T: Copyable & Deinitable](mut into: List[T], value: T):
    into.append(value.copy())


def main():
    var xs: List[Int] = [1, 2, 3]
    var m = List[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    keep(m, p)
    xs.append(9)
    print(len(m), m[0][][1])
