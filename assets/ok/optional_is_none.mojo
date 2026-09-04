# `is` / `is not` are comparison operators dispatching to the left operand's
# `__is__` / `__isnot__` (current Mojo's identity dunders): Optional compares
# against `None`, in plain and chained-`not` positions, and in conditions.
def describe(x: Optional[Int]) -> String:
    if x is None:
        return String("empty")
    if x is not None:
        return String("present")
    return String("unreachable")

def main():
    var a: Optional[Int] = 5
    var empty = Optional[Int]()
    print(a is None, a is not None)
    print(empty is None, empty is not None)
    print(describe(a), describe(None))
    print(not (a is None))
    var count = 0
    var opt: Optional[Int] = 3
    while opt is not None:
        count += opt.take()
    print(count)
