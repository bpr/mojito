# A generic `def` its template serves stores a copy of its argument, which no
# argument expression hands over. The list's type names `xs`, so the list
# keeps `xs` alive while it is read; that loan grants no access of its own,
# so `xs.append(9)` beside it runs, as at the pin, and the pointer the list
# holds reads the appended list.
def keep[T: Copyable & Deinitable](mut into: List[T], value: T):
    into.append(value.copy())


def main():
    var xs: List[Int] = [1, 2, 3]
    var m = List[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    keep(m, p)
    xs.append(9)
    print(len(m), m[0][][1])
