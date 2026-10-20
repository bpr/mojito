# expect: has no method '__getitem__'
# A generic subject bounded by `Sized` alone has no element getter.
def first[T: Sized](x: T) -> Int:
    match x:
        case [a]:
            return a
        case _:
            return -1


def main():
    var xs: List[Int] = [1]
    print(first(xs))
