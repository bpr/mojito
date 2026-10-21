# expect: expected a tuple
# A starred tuple target unpacks the slice, a `Span`, which is not a tuple.
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [f, *(a, b)] = values
    print(f)
