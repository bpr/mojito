# expect: expected a 3-element tuple
# A tuple loop target must match the arity of each element.
def main():
    var pairs: List[Tuple[Int, String]] = [(1, "a")]
    for a, b, c in pairs:
        print(a, b, c)
