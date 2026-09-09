# Subscript writes through a parametric-mut ref field (`Origin[mut=m]`) are
# accepted inside the generic body and judged per instantiation: a receiver
# whose origin binds a mutable source may write through the view, and the
# write lands in the borrowed storage.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def bump(mut self):
        self.src[0] += 1

    def put(mut self, x: Int):
        self.src[0] = x

    def first(self) -> Int:
        return self.src[0]

def main():
    var data = List[Int]()
    data.append(7)
    var v = View(data)
    v.bump()
    print(v.first())
    v.put(20)
    print(v.first())
    print(data[0])
