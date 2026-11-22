# expect: expression must be mutable for in-place operator destination
# An in-place store through a parametric-mut ref field (`Origin[mut=m]`)
# rejects in the method body, as upstream judges the binder there: `m` is
# not provably `True`, however a caller binds it.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def bump(mut self):
        self.src[0] += 1

def poke(mut data: List[Int]):
    var v = View(data)
    v.bump()

def main():
    var source = List[Int]()
    source.append(7)
    poke(source)
