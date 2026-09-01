# expect: cannot be implicitly copied
# `var b = a` implicitly copies `a`; a `List` place needs `a^` or `a.copy()`.
def main():
    var a: List[Int] = [1, 2]
    var b = a
    print(len(b))
