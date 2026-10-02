# expect: cannot be implicitly copied
# A place handed to an owned `var *xs` collector is copied into the pack, so
# its type must be `ImplicitlyCopyable`, as at a named `var` parameter:
# transfer it with `^` or spell `.copy()`.
def take(var *xs: List[Int]) -> Int:
    return 1


def main():
    var xs: List[Int] = [1, 2]
    print(take(xs))
    print(len(xs))
