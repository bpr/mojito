# expect: returned reference escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Sink:
    var slot: RefBox

    def __iadd__(mut self, var box: RefBox):
        self.slot = box^

def make(mut keep: List[Int]) -> Sink:
    ref whole = keep
    var sink = Sink(RefBox(whole))
    var local = [9]
    ref alias = local
    sink += RefBox(alias)
    return sink^

def main():
    var keep = [1]
    var got = make(keep)
