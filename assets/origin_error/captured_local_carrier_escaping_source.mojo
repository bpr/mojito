# expect: escapes storage
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
    def push() {mut sink}:
        var inner: List[Int] = [5]
        ref a = inner
        sink.slot = RefBox(a)
    push()
    print(sink.slot.value[0])
