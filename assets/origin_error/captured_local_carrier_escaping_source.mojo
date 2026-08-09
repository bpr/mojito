# expect: escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier:
    var slot: RefBox

def main():
    var keep = [1]
    ref whole = keep
    var sink = Carrier(RefBox(whole))
    def push() {mut sink}:
        var inner = [5]
        ref a = inner
        sink.slot = RefBox(a)
    push()
    print(sink.slot.value[0])
