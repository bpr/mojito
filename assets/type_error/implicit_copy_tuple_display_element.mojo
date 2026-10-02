# expect: cannot be implicitly copied
# A tuple display hands its elements to `Tuple`'s owned `var *args`
# collector, so a place element is copied and its type must be
# `ImplicitlyCopyable`.
def main():
    var xs: List[Int] = [1, 2]
    var t = (xs, 1)
    print(len(xs))
