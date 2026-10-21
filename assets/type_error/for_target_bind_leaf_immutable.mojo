# expect: expression must be mutable in assignment
# A loop target's bare name binds its piece immutably (upstream's `Bind`).
def main():
    var pairs: List[Tuple[Int, String]] = [(1, "a")]
    for a, b in pairs:
        a = 5
        print(a, b)
