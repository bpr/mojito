# expect: escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

@fieldwise_init
struct Stasher(def(mut Carrier, RefBox)):
    var count: Int
    def __call__(mut self, mut sink: Carrier, box: RefBox):
        self.count += 1
        sink.slot = box^

def make(mut source: List[Int]) -> Carrier:
    ref src_alias = source
    var sink = Carrier(RefBox(src_alias))
    var s = Stasher(0)
    var local: List[Int] = [9]
    ref alias = local
    s(sink, RefBox(alias))
    return sink^

def main():
    var source: List[Int] = [7]
    var got = make(source)
