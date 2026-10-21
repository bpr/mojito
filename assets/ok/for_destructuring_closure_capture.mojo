# Each iteration's destructured bindings are that iteration's own: a
# `{var a}` lambda copies the current iteration's leaf, and a `{mut a}`
# nested def writes the iteration's `var` leaf, as on the pin.
def main():
    var pairs: List[Tuple[Int, String]] = [(1, "a"), (2, "b")]
    var total = 0
    for (var a, b) in pairs:
        total = total + (lambda {var a} -> Int: a * 10)()
        print(b, total)
    for (var a, b) in pairs:
        def bump() {mut a}:
            a += 1
        bump()
        print(a, b, pairs[0][0])
