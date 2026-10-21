# expect: use of uninitialized value 'pairs'
# A loop target's leaves borrow the iterated list, which the body cannot move.
def main():
    var pairs: List[Tuple[Int, String]] = [(1, "a")]
    for a, b in pairs:
        var moved = pairs^
        print(a, b, len(moved))
