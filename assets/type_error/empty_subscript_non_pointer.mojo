# The empty subscript calls a zero-argument `__getitem__`, which `List`
# does not declare.
# expect: no overload matches
def main():
    var xs: List[Int] = [1, 2]
    print(xs[])
