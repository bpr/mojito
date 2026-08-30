# Upstream's origin placeholder spellings (`_`, `...`) mark an origin slot
# explicitly inferred: an initialized local infers the origin from its
# initializer, and the application counts as complete (no partial-application
# rejection). Both the stdlib Span and a user origin-generic struct accept
# them.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

def main():
    var xs = List[Int]()
    xs.append(7)
    xs.append(9)
    var s: Span[Int, _] = xs
    print(s[0])
    var t: Span[Int, ...] = xs
    print(t[1])
    ref r = xs
    var v: EntryIter[_] = EntryIter(r, 0)
    print(v.src[0])
