# expect: cannot be implicitly copied
# `List.__add__(self, var other: Self)` binds its operand like any owned
# argument: `p + q` needs `p + q^` or `p + q.copy()`.
def main():
    var p: List[Int] = [1]
    var q: List[Int] = [2]
    var r = p + q
    print(len(r))
