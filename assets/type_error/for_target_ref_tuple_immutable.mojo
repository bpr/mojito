# expect: expression must be mutable in assignment
# `for ref (a, b)` binds through the immutable per-iteration binder, so `a` is read-only.
def main():
    var pairs: List[Tuple[Int, String]] = [(1, "a")]
    for ref (a, b) in pairs:
        a = 5
        print(a, b)
