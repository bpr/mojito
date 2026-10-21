# expect: does not conform to 'Sized'
# A star's slice is untyped, as an element is, so a sequence pattern over
# it is rejected.
def main():
    var values: List[Int] = [1, 2, 3, 4, 5]
    match values:
        case [f, *[a, b, c, d]]:
            print(f, a)
        case _:
            pass
