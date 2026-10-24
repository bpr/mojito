# expect: use of invalidated interior reference 'lst["element"]'
# requires: stdlib
# A by-value `List` element argument is read at call setup, after a later
# argument's `mut` pass may have reallocated the list's storage: the pin
# rejects the stale element reference.
def bl(mut l: List[Int]) -> Int:
    l[0] += 100
    return 0


def two(a: Int, b: Int):
    print("two", a, b)


def main():
    var lst: List[Int] = [1, 2]
    two(lst[0], bl(lst))
