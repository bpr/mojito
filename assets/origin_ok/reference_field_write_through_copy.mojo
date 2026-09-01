# A Copyable value written through a `ref` field runs the referent's copy
# lifecycle (spelled explicitly, since `List` is not ImplicitlyCopyable): the
# referent owns independent storage, so the source's later drop cannot free
# it.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]
    def retarget(mut self, mut source: List[Int]):
        self.value = source.copy()

def main():
    var first: List[Int] = [1, 2]
    ref whole = first
    var box = RefBox(whole)
    var second: List[Int] = [7, 8]
    box.retarget(second)
    print(box.value[0])
