# A read argument of a register-passable element is a copy, not an alias: a
# call over `xs[0]` (an `Int`) assigned back to `xs` is accepted. The
# pinned Mojo prints `1`.
def rebuild(x: Int) -> List[Int]:
    return [x, x]

def main():
    var xs: List[Int] = [1, 2]
    xs = rebuild(xs[0])
    print(xs[1])
