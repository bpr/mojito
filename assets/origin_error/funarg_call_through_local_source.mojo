# expect: escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Carrier:
    var slot: RefBox

def stash(mut sink: Carrier, box: RefBox):
    sink.slot = box^

def feed[callback: def(mut Carrier, RefBox) thin](mut sink: Carrier):
    var local = [5]
    ref alias = local
    callback(sink, RefBox(alias))

def main():
    var keep = [1]
    ref whole = keep
    var sink = Carrier(RefBox(whole))
    feed[stash](sink)
    print(sink.slot.value[0])
