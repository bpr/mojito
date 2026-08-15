# Mojito's Span is single-parameter (`Span[Int]`, origin solved internally),
# while the audited head requires the origin slot (`Span[Int, _]`) and
# rejects the bare spelling — a recorded acceptance divergence until the
# span parameterization (mut/origin parameters, `_` unbinding, Imm/Mut
# aliases) is implemented.
def total(s: Span[Int]) -> Int:
    var acc = 0
    var i = 0
    while i < len(s):
        acc += s[i]
        i += 1
    return acc

def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    print(total(xs))
