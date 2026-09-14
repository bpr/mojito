# A list element is an owned interior of the list: passing `xs[0]` (a
# `String`, read by reference) to a call assigned back to `xs` aliases the
# result. The pinned Mojo rejects it with the same text.
# expect: aliasing values passed immutably to 'x' argument and constructed as a result in 'rebuild' call
def rebuild(x: String) -> List[String]:
    return [x, x]

def main():
    var xs: List[String] = [String("a"), String("b")]
    xs = rebuild(xs[0])
    print(xs[1])
