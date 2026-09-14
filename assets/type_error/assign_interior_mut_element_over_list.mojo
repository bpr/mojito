# A `mut` argument always aliases, even of a register-passable element: the
# call writes through `xs[0]` while its result replaces `xs`. The pinned
# Mojo rejects it with the same text (it says "immutably" here too).
# expect: aliasing values passed immutably to 'x' argument and constructed as a result in 'rebuild' call
def rebuild(mut x: Int) -> List[Int]:
    x += 1
    return [x, x]

def main():
    var xs: List[Int] = [1, 2]
    xs = rebuild(xs[0])
    print(xs[1])
