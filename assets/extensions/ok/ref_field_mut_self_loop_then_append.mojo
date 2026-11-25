# A `mut self` method may iterate a `ref` field and append through the same
# field once the loop is over: the loop's interior generation ends with the
# loop, so the write invalidates nothing live.
@fieldwise_init
struct Walk[o: MutOrigin]:
    var src: ref[Self.o] List[Int]

    def dup(mut self):
        var total = 0
        for x in self.src:
            total += x
        self.src.append(total)

def main():
    var l: List[Int] = [1, 2]
    var w = Walk(l)
    w.dup()
    print(len(l))
    print(l[2])
