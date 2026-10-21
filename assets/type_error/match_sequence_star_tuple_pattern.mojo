# expect: expected a tuple type to match against
# A star's slice is untyped, as an element is, so a tuple pattern over it is
# rejected.
def main():
    var values: List[Int] = [1, 2, 3, 4, 5]
    match values:
        case [f, *(a, b)]:
            print(f)
        case _:
            pass
