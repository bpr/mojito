# expect: escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

def stash(mut sink: Carrier, box: RefBox):
    sink.slot = box^

def feed(f: def(mut Carrier, RefBox), mut sink: Carrier, box: RefBox):
    f(sink, box^)

def make(mut source: List[Int]) -> Carrier[origin_of(source)]:
    ref src_alias = source
    var sink = Carrier(RefBox(src_alias))
    var local: List[Int] = [9]
    ref alias = local
    feed(stash, sink, RefBox(alias))
    return sink^

def main():
    var source: List[Int] = [7]
    var got = make(source)
