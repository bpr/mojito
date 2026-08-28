@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var sink = Carrier(RefBox(whole))
    var local: List[Int] = [9]
    def push() {mut sink, mut local}:
        ref alias = local
        sink.slot = RefBox(alias)
    push()
    print(sink.slot.value[0])
