# Both compilers reject a partial Span application in a parameter
# annotation (`s: Span[Int]` — the origin slot is omitted and a parameter
# has no initializer to infer it from): the a79fbdf59f2 pin reports
# "'Span' failed to infer parameter 'origin', specify the parameter or use
# '_' or '...' to unbind the parameter explicitly", and Mojito reports the
# same placeholder hint (parameter-annotation tightening, 2026-08-29).
# The accepted spelling is `Span[Int, _]`.
def total(s: Span[Int]) -> Int:
    var acc = 0
    var i = 0
    while i < len(s):
        acc += s[i]
        i += 1
    return acc

def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    print(total(xs))
