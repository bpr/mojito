# An instance of a bundled template whose argument carries a loan
# (`List[Span[Int, origin_of(xs)]]`, `Dict[Int, Span[Int, origin_of(xs)]]`)
# clones its methods as a user template's instance does: each origin slot of
# the argument becomes an origin binder the clone declares. A clone of a
# certified template derives its facts from the checked template, so
# `List.__imul__`'s `self.extend(orig.copy())` is not checked again at the
# concrete argument. A store of the element parameter, which the template
# records as a latent transfer (`List.append`'s `value^`), is published by
# the instance, so the list keeps the loan it was given
# (`docs/notes/instantiation-from-template.md`, obligation 14).
def main() raises:
    var xs: List[Int] = [1, 2, 3]
    var l = List[Span[Int, origin_of(xs)]]()
    l.append(Span(xs))
    l.insert(0, Span(xs))
    l *= 2
    print(len(l), l[0][1])
    var total = 0
    for s in l:
        total += s[2]
    print(total)
    var c = l.copy()
    var last = c.pop()
    print(len(c), last[0])
    var d = Dict[Int, Span[Int, origin_of(xs)]]()
    d[1] = Span(xs)
    d[2] = Span(xs)
    var sum = 0
    for e in d.items():
        sum += e.key + e.value[2]
    print(len(d), 1 in d, sum)
    print(xs[0])
