# A field store into, and a `mut self` method call on, the element a
# parametric-mut ref field (`Origin[mut=m]`) reaches are writes through the
# view, judged per instantiation like `self.src[0] += 1`: over a mutable
# source, both land in the borrowed storage.
@fieldwise_init
struct C(Copyable, Movable):
    var n: Int

    def inc(mut self):
        self.n += 1

@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[C]

    def put(mut self, x: Int):
        self.src[0].n = x

    def bump(mut self):
        self.src[0].inc()

    def first(self) -> Int:
        return self.src[0].n

def main():
    var data = List[C]()
    data.append(C(7))
    var v = View(data)
    v.put(3)
    print(v.first())
    v.bump()
    print(v.first())
    print(data[0].n)
# stdout: 3
# stdout: 4
# stdout: 4
