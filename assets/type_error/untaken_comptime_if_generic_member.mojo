# expect: type 'T' has no method 'nonexistent'
# A member the type parameter's bounds do not declare is rejected inside an
# untaken `comptime if` arm: the arm is checked with `T` symbolic, so a
# concrete caller cannot make the template valid.
def g[T: Copyable, flag: Bool](x: T) -> String:
    comptime if flag:
        return "ok"
    else:
        x.nonexistent()
        return "bad"


def main():
    print(g[Int, True](1))
