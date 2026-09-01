# expect: cannot be implicitly copied
# A place of a Copyable-only type cannot be passed to a `var` parameter
# implicitly, even at its last use: transfer it with `^` or spell `.copy()`.
def take(var values: List[Int]):
    print(len(values))

def main():
    var values: List[Int] = [1]
    take(values)
