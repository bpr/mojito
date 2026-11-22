# Subscript writes through a ref field over a `MutOrigin` binder are
# accepted in the method body, where the binder is mutable, and land in the
# borrowed storage.
@fieldwise_init
struct View[o: MutOrigin]:
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
