# expect: returned reference escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Sink[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

    def __iadd__(mut self, var box: RefBox):
        self.slot = box^

def make(mut keep: List[Int]) -> Sink[origin_of(keep)]:
    ref whole = keep
    var sink = Sink(RefBox(whole))
    var local: List[Int] = [9]
    ref alias = local
    sink += RefBox(alias)
    return sink^

def main():
    var keep: List[Int] = [1]
    var got = make(keep)
